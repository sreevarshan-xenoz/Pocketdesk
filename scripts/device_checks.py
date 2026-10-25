#!/usr/bin/env python3
"""Owed device checks that run by script (N1).

Runs checks from docs/device-verification-checklist.md on a test phone over
adb, against a throwaway server that is already paired with that phone:
`--server-root` is a folder holding `state/` and `config/` (made once by
pairing by hand). The server runs with the clipboard seam, the phone's input
off in its trust store, and `adb reverse` from the phone's saved port to the
throwaway's. Every wait has a deadline. Pocketdesk is never force-stopped
(on ColorOS that turns its Accessibility service off): a check whose switch
is off on the phone is SKIPped, saying what to turn on.

    python scripts/device_checks.py --server-root DIR --server-exe PATH [--only ID ...]
    python scripts/device_checks.py --list

The fixture app (android/fixture) must be installed on the phone. Exit 0
when no check FAILed or TIMEd OUT.
"""

from __future__ import annotations

import argparse
import hashlib
import html
import http.client
import json
import os
import pathlib
import re
import secrets
import shlex
import shutil
import stat
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass, field
from typing import Callable, Optional

APP = "dev.pocketdesk.android"
FIXTURE = "dev.pocketdesk.fixture"
DUMP = "/data/local/tmp/device_checks.xml"
# A moment more once the control service says it is back.
SETTLE = 0.5


# ---- pure parts (tested in scripts/test_device_checks.py) ----


def linked_ids(mirror: str) -> list[str]:
    """The phones a `live_sessions.json` mirror lists as linked."""
    try:
        data = json.loads(mirror)
    except ValueError:
        return []
    return [p.get("device_id", "") for p in data.get("linked", []) if p.get("device_id")]


def events_after(lines: list[str], after_unix: int, kind: str) -> list[dict]:
    """The events of `kind` in an `events.jsonl` at or after `after_unix`."""
    out = []
    for line in lines:
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("event") == kind and int(event.get("at", 0)) >= after_unix:
            out.append(event)
    return out


def media_session_shows(dump: str, package: str, title: str) -> bool:
    """Whether `dumpsys media_session` lists an active session of `package`
    whose metadata names `title` (N5)."""
    # Each session starts on a line indented four spaces; its details six.
    for block in re.split(r"\n {4}(?=\S)", dump)[1:]:
        if f"package={package}" in block and "active=true" in block and title in block:
            return True
    return False


def marked_folder(prefs_json: str) -> tuple[Optional[str], Optional[str]]:
    """The names of a shared folder the phone marked for the computer to add
    to, and of one it did not (N6), from its `folders` preference."""
    try:
        folders = json.loads(prefs_json or "[]")
    except ValueError:
        return None, None
    marked = next((f["name"] for f in folders if f.get("add")), None)
    unmarked = next((f["name"] for f in folders if not f.get("add")), None)
    return marked, unmarked


def dav_children(xml: str, base: str) -> list[str]:
    """The names a PROPFIND of `base` lists right below it, decoded."""
    from urllib.parse import unquote
    out = []
    for href in re.findall(r"<D:href>([^<]+)</D:href>", xml):
        path = unquote(html.unescape(href))
        if path.startswith(base) and path != base:
            rest = path[len(base):].strip("/")
            if rest and "/" not in rest:
                out.append(rest)
    return out


def drive_root(roots: list[str], name: Optional[str]) -> Optional[str]:
    """The drive's folder for the phone's shared folder `name`: the name, or
    with its id after it when two share it (N6 review)."""
    if name is None:
        return None
    return next((r for r in roots if r == name or (r.startswith(name + " (") and r.endswith(")"))), None)


def dav(port: int, method: str, path: str, body: bytes = b"", depth: Optional[str] = None) -> tuple[int, bytes]:
    """A WebDAV request to the drive on loopback, within a minute."""
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
    try:
        headers = {"Content-Length": str(len(body))}
        if depth is not None:
            headers["Depth"] = depth
        conn.request(method, path, body=body, headers=headers)
        response = conn.getresponse()
        return response.status, response.read()
    finally:
        conn.close()


def field_text(dump: str, description: str = "fixture-field") -> Optional[str]:
    """The text of the view `uiautomator dump` names `description`."""
    for node in re.findall(r"<node [^>]*>", dump):
        if f'content-desc="{description}"' in node:
            found = re.search(r' text="([^"]*)"', node)
            return html.unescape(found.group(1)) if found else ""
    return None


def dump_nodes(dump: str) -> list[tuple[str, int, int]]:
    """Each view's text with its centre, top to bottom as dumped."""
    out = []
    for node in re.findall(r"<node [^>]*>", dump):
        text = re.search(r' text="([^"]*)"', node)
        box = re.search(r'bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', node)
        if text and box:
            x1, y1, x2, y2 = map(int, box.groups())
            out.append((html.unescape(text.group(1)), (x1 + x2) // 2, (y1 + y2) // 2))
    return out


def connect_button(dump: str, address: str) -> Optional[tuple[int, int]]:
    """Where to tap Connect on Home's card for the computer at `address`:
    the first Connect below the line that starts with it (W1)."""
    nodes = dump_nodes(dump)
    card = next((y for text, _, y in nodes if text.startswith(address + " ")), None)
    if card is None:
        return None
    return next(((x, y) for text, x, y in nodes if text == "Connect" and y > card), None)


def hud_caption(dump: str) -> Optional[str]:
    """The session HUD's basis caption, `rates over … · rtt …` (§2d)."""
    return next((text for text, _, _ in dump_nodes(dump) if text.startswith("rates over ")), None)


def rtt_count(caption: str) -> Optional[int]:
    """How many RTT polls the caption says its median is of; 0 before the
    first."""
    if "rtt not measured yet" in caption:
        return 0
    found = re.search(r"rtt median of (\d+)", caption)
    return int(found.group(1)) if found else None


def log_seconds(line: str) -> Optional[float]:
    """A logcat line's time of day, in seconds."""
    found = re.match(r"\d\d-\d\d (\d\d):(\d\d):(\d\d\.\d+) ", line)
    if not found:
        return None
    hours, minutes, seconds = found.groups()
    return int(hours) * 3600 + int(minutes) * 60 + float(seconds)


def prefs(xml: str) -> dict[str, object]:
    """A SharedPreferences file's booleans, strings and string sets."""
    out: dict[str, object] = {}
    for name, value in re.findall(r'<boolean name="([^"]+)" value="(true|false)"', xml):
        out[name] = value == "true"
    for name, value in re.findall(r'<string name="([^"]+)">([^<]*)</string>', xml):
        out[name] = html.unescape(value)
    for name, body in re.findall(r'<set name="([^"]+)">(.*?)</set>', xml, re.S):
        out[name] = [html.unescape(s) for s in re.findall(r"<string>([^<]*)</string>", body)]
    return out


def owner_state_dirs() -> list[pathlib.Path]:
    """Where the owner's own server keeps its state and config, never to be
    used here: the new and the legacy (`linux-link`) names, and a state
    folder the environment names."""
    bases = []
    for var in ("LOCALAPPDATA", "APPDATA", "XDG_STATE_HOME", "XDG_CONFIG_HOME"):
        if os.environ.get(var):
            bases.append(pathlib.Path(os.environ[var]))
    bases += [pathlib.Path.home() / ".local" / "state", pathlib.Path.home() / ".config"]
    dirs = [b / name for b in bases for name in ("pocketdesk", "linux-link")]
    for var in ("POCKETDESK_STATE_DIR", "POCKETDESK_CONFIG_DIR"):
        if os.environ.get(var):
            dirs.append(pathlib.Path(os.environ[var]))
    return dirs


def owner_control_port() -> int:
    """The control port the owner's own server listens on (1716 unless its
    config says otherwise)."""
    for folder in owner_state_dirs():
        config = folder / "config.toml"
        if config.exists():
            return control_port(config.read_text(encoding="utf-8", errors="replace"))
    return 1716


def is_owner_root(root: pathlib.Path, owners: list[pathlib.Path]) -> bool:
    """Whether `root`, or its state or config folder, is the owner's own."""
    mine = {(root / sub).resolve() for sub in ("", "state", "config")}
    return any(o.resolve() in mine for o in owners)


def fresh_linked(mirror: str, now_unix: float, within: float = 10) -> list[str]:
    """The linked phones of a mirror written in the last `within` seconds: a
    stale one (a server since stopped) links no one (N1 review)."""
    try:
        updated = json.loads(mirror).get("updated_unix", 0)
    except ValueError:
        return []
    return linked_ids(mirror) if now_unix - updated <= within else []


def status_running(out: str) -> bool:
    """Whether `pocketdesk status --json`'s output says the server runs; its
    log lines (some with braces) may come first."""
    for line in out.splitlines():
        if line.startswith('{"api"'):
            try:
                return bool(json.loads(line).get("server", {}).get("running"))
            except ValueError:
                return False
    return False


def control_port(config_toml: str, default: int = 1716) -> int:
    found = re.search(r"^\s*control_port\s*=\s*(\d+)", config_toml, re.M)
    return int(found.group(1)) if found else default


def with_config(config_toml: str, values: dict[str, str]) -> str:
    """`config_toml` with each top-level key in `values` set to its TOML
    value, at the file's top; a key under a table is left alone, and an
    array spread over lines goes whole."""
    kept: list[str] = []
    in_table = False
    # Inside a multi-line array: whether it is dropped. It ends at a line
    # that starts with `]`, as the config's writer lays one out (a `]`
    # inside a string does not end it).
    array: Optional[bool] = None
    for line in config_toml.splitlines(keepends=True):
        if array is not None:
            if array:
                kept.append(line)
            if line.lstrip().startswith("]"):
                array = None
            continue
        if re.match(r"\s*\[", line):
            in_table = True
        key = re.match(r"\s*([A-Za-z0-9_]+)\s*=\s*(.*?)\s*$", line)
        opens = bool(key) and key.group(2).startswith("[") and not key.group(2).endswith("]")
        if key and not in_table and key.group(1) in values:
            array = False if opens else None
            continue
        kept.append(line)
        if opens:
            array = True
    head = "".join(f"{k} = {v}\n" for k, v in values.items())
    return head + "".join(kept)


def alarm_vibrations(dump: str, package: str) -> list[tuple[str, int]]:
    """`package`'s finished alarm vibrations in `dumpsys vibrator_manager`:
    when each began (to the second) and how long it ran, in ms."""
    found = []
    for line in dump.splitlines():
        if f"opPkg={package}," not in line or "Usage=ALARM" not in line:
            continue
        when = re.search(r"createTime: (\d\d-\d\d \d\d:\d\d:\d\d)", line)
        ran = re.search(r"durationMs: (\d+)", line)
        if when and ran:
            found.append((when.group(1), int(ran.group(1))))
    return found


def torch_turns(dump: str, since: str) -> list[tuple[str, str]]:
    """The torch turned on or off at `since` or after (`MM-DD HH:MM:SS`,
    the phone's clock), oldest first, from `dumpsys media.camera` (which
    lists the newest first; two in one second keep their order)."""
    turns = re.findall(r"(\d\d-\d\d \d\d:\d\d:\d\d) : Torch for camera id \S+ turned (on|off)", dump)
    return [(when, how) for when, how in reversed(turns) if when >= since]


# ---- the phone and the server ----


def run(cmd: list[str], timeout: float = 30, env: Optional[dict] = None) -> tuple[int, str]:
    full = dict(os.environ)
    full["MSYS_NO_PATHCONV"] = "1"
    if env:
        full.update(env)
    try:
        done = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout, env=full)
    except subprocess.TimeoutExpired:
        return 124, "TIMEOUT"
    return done.returncode, (done.stdout or "") + (done.stderr or "")


def adb(*args: str, timeout: float = 30) -> str:
    return run(["adb", *args], timeout=timeout)[1]


def device_command(*args: str) -> str:
    """`args` as one command line for the phone's shell, each quoted: `adb
    shell` joins its arguments, so an empty one vanished and one with spaces
    split."""
    return " ".join(shlex.quote(a) for a in args)


def am(*args: str) -> str:
    return adb("shell", device_command("am", *args))


def wait_for(probe: Callable[[], object], timeout: float, step: float = 0.5) -> object:
    """`probe`'s first true answer within `timeout` seconds, else None."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        got = probe()
        if got:
            return got
        time.sleep(step)
    return None


class Phone:
    def wake(self) -> None:
        adb("shell", "input", "keyevent", "KEYCODE_WAKEUP")
        if self.locked():
            adb("shell", "wm", "dismiss-keyguard")
            # Only on the lock screen: a swipe elsewhere lands on the app in front.
            if self.locked():
                adb("shell", "input", "swipe", "540", "2100", "540", "500", "250")
        wait_for(lambda: not self.locked(), 5)

    def sleep(self) -> None:
        adb("shell", "input", "keyevent", "KEYCODE_SLEEP")
        wait_for(self.locked, 5)

    def locked(self) -> bool:
        return "isKeyguardShowing=true" in adb("shell", "dumpsys", "window")

    def sdk(self) -> int:
        out = adb("shell", "getprop", "ro.build.version.sdk").strip()
        return int(out) if out.isdigit() else 0

    def prefs(self, name: str) -> dict[str, object]:
        out = adb("shell", f"run-as {APP} cat shared_prefs/{name}.xml")
        if "run-as:" in out and "not debuggable" in out:
            raise RuntimeError("Pocketdesk is not a debug build: its settings cannot be read")
        return prefs(out)

    def may_notify(self, package: str) -> bool:
        return "POST_NOTIFICATIONS: granted=true" in adb("shell", "dumpsys", "package", package)

    def accessibility_on(self) -> bool:
        return f"{APP}/{APP}.control.ControlService" in adb("shell", "settings", "get", "secure", "enabled_accessibility_services")

    def dump(self) -> str:
        """The screen's views. `uiautomator` suppresses other Accessibility
        services while it reads, so Pocketdesk's control service is away for
        a moment after: see [service_back]."""
        adb("logcat", "-c")
        adb("shell", "uiautomator", "dump", DUMP)
        return adb("shell", "cat", DUMP)

    def service_back(self, timeout: float = 15) -> bool:
        """Pocketdesk's control service connected again after the last dump:
        its own "control service connected" in logcat (about 3 s on the OPPO)."""
        back = wait_for(lambda: "control service connected" in self.logcat("ControlService"), timeout)
        time.sleep(SETTLE)
        return bool(back)

    def field(self, text: str = "") -> bool:
        """The fixture's field in front, holding `text`; tried three times
        (the first dump after waking can miss it)."""
        for _ in range(3):
            am("start", "-W", "-S", "-n", f"{FIXTURE}/.FieldActivity", "--es", "text", text)
            if wait_for(lambda: field_text(self.dump()) == text, 10):
                return True
            self.wake()
        return False

    def copy(self, text: str, sensitive: bool = False) -> None:
        am("start", "-n", f"{FIXTURE}/.CopyActivity", "--es", "text", text, "--ez", "sensitive", "true" if sensitive else "false")

    def play(self, title: str, playing: bool = True) -> None:
        """The fixture's silent player, with a cover (N2)."""
        am("start-foreground-service", "-n", f"{FIXTURE}/.PlayerService",
           "--es", "title", title, "--ez", "playing", "true" if playing else "false")

    def stop_playing(self) -> None:
        am("start-foreground-service", "-n", f"{FIXTURE}/.PlayerService", "-a", "dev.pocketdesk.fixture.STOP")

    def notify(self, nid: int, title: str, text: str, style: str = "plain", progress: int = 0) -> None:
        am("broadcast", "-n", f"{FIXTURE}/.NotifyReceiver", "-a", "dev.pocketdesk.fixture.NOTIFY",
           "--ei", "id", str(nid), "--es", "title", title, "--es", "text", text, "--es", "style", style,
           "--ei", "progress", str(progress))

    def cancel(self, nid: int) -> None:
        am("broadcast", "-n", f"{FIXTURE}/.NotifyReceiver", "-a", "dev.pocketdesk.fixture.CANCEL", "--ei", "id", str(nid))

    def logcat(self, tag: str) -> str:
        return adb("logcat", "-d", "-s", f"{tag}:*")

    def clock(self) -> str:
        """The phone's time as its dumpsys lines write it, `MM-DD HH:MM:SS`."""
        return adb("shell", device_command("date", "+%m-%d %H:%M:%S")).strip()

    def granted(self, permission: str) -> bool:
        return f"{permission}: granted=true" in adb("shell", "dumpsys", "package", APP)

    def shows_media(self, title: str) -> bool:
        """Pocketdesk's own media session shows `title` (N5)."""
        return media_session_shows(adb("shell", "dumpsys", "media_session"), APP, title)


@dataclass
class Server:
    root: pathlib.Path
    exe: pathlib.Path
    child: Optional[subprocess.Popen] = None
    # More seams for one check (AM1), read at the next start.
    extra: dict = field(default_factory=dict)

    def env(self) -> dict:
        return self.extra | {
            "COMPUTERNAME": "Test Desk",
            "POCKETDESK_STATE_DIR": str(self.root / "state"),
            "POCKETDESK_CONFIG_DIR": str(self.root / "config"),
            "POCKETDESK_CLIPBOARD_FILE": str(self.root / "clip.txt"),
            # AD1: the clipboard's image, a PNG (none while it is absent).
            "POCKETDESK_CLIPBOARD_IMAGE_FILE": str(self.root / "clip.png"),
            # N5: what plays here, read from a file, so this box's players are untouched.
            "POCKETDESK_MEDIA_FILE": str(self.root / "computer_media.json"),
            # P3: presenting while this file is there.
            "POCKETDESK_PRESENTING_FILE": str(self.root / "presenting"),
            # P7: the files copied here, one path a line.
            "POCKETDESK_CLIPBOARD_FILES_FILE": str(self.root / "clip_files.txt"),
            # X1 review: what phones send lands in the throwaway's own folder,
            # whatever its config says, never this computer's Downloads.
            "POCKETDESK_DOWNLOAD_DIR": str(self.root / "downloads"),
            # AD2: the phone's file browser shows this folder as home, never
            # the owner's.
            "POCKETDESK_BROWSE_HOME": str(self.root / "home"),
            # AG1: a lock is a line in this file, never this computer's lock.
            "POCKETDESK_LOCK_FILE": str(self.root / "lock.txt"),
        }

    def cli(self, *args: str) -> tuple[int, str]:
        return run([str(self.exe), *args], env=self.env())

    def configure(self, values: dict[str, str]) -> str:
        """Set top-level config keys while it runs (it reads them at each
        notification); the file as it was, to put back."""
        path = self.root / "config" / "config.toml"
        before = path.read_text(encoding="utf-8") if path.exists() else ""
        tmp = path.with_suffix(".toml.tmp")
        tmp.write_text(with_config(before, values), encoding="utf-8")
        os.replace(tmp, path)
        return before

    def append_config(self, text: str) -> str:
        """Tables added at the config's end while it runs (R1); the file
        as it was, to put back."""
        path = self.root / "config" / "config.toml"
        before = path.read_text(encoding="utf-8") if path.exists() else ""
        tmp = path.with_suffix(".toml.tmp")
        tmp.write_text(before.rstrip("\n") + "\n" + text, encoding="utf-8")
        os.replace(tmp, path)
        return before

    def events_of(self, count: int, kind: str, device: str) -> list[dict]:
        """`device`'s events of `kind` after the log's first `count` lines."""
        path = self.root / "state" / "events.jsonl"
        if not path.exists():
            return []
        lines = path.read_text(encoding="utf-8").splitlines()[count:]
        return [e for e in events_after(lines, 0, kind) if e.get("device_id") == device]

    def restore_config(self, before: str) -> None:
        path = self.root / "config" / "config.toml"
        tmp = path.with_suffix(".toml.tmp")
        tmp.write_text(before, encoding="utf-8")
        os.replace(tmp, path)

    def ask_phone(self, device: str, action: str, app: Optional[str] = None) -> None:
        """As the desktop app asks (core::local::phone_actions)."""
        folder = self.root / "state" / "phone_actions_out"
        folder.mkdir(exist_ok=True)
        rid = str(uuid.uuid4())
        body = {"id": rid, "device_id": device, "action": action, "created_unix": int(time.time())}
        if app:
            body["app"] = app
        tmp = folder / f"{rid}.json.tmp"
        tmp.write_text(json.dumps(body), encoding="utf-8")
        os.replace(tmp, folder / f"{rid}.json")

    def send_clipboard(self, device: str) -> None:
        """As the tray's Send the clipboard asks (core::local::clip_history::request_current)."""
        folder = self.root / "state" / "clipboard_out"
        folder.mkdir(exist_ok=True)
        rid = str(uuid.uuid4())
        body = {"id": rid, "device_id": device, "text": "", "created_unix": int(time.time()), "current": True}
        tmp = folder / f"{rid}.json.tmp"
        tmp.write_text(json.dumps(body), encoding="utf-8")
        os.replace(tmp, folder / f"{rid}.json")

    def kept(self, name: str, device: str) -> Optional[dict]:
        """`device`'s entry in a `{device_id: …}` file the server keeps."""
        try:
            kept = json.loads((self.root / "state" / name).read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None
        return kept.get(device) if isinstance(kept, dict) else None

    def running(self) -> bool:
        _, out = run([str(self.exe), "status", "--json"], env=self.env())
        return status_running(out)

    def input_off(self) -> None:
        """No phone gives this computer input during the checks: written
        before the server starts, in one move."""
        path = self.root / "state" / "trusted_devices.json"
        store = json.loads(path.read_text(encoding="utf-8"))
        for device in store.get("trusted_device_ids", []):
            mine = store.setdefault("permissions", {}).setdefault(device, {})
            mine["input"] = False
            # AG1 review: nor a lock left on by a run cut short (a throwaway
            # started later by hand, without the seam, would lock this box).
            mine.pop("away_lock", None)
        tmp = path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(store, indent=2), encoding="utf-8")
        os.replace(tmp, path)

    def log(self) -> str:
        path = self.root / "device_checks.log"
        return path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""

    def start(self) -> Optional[str]:
        """Start it, with every phone's input off; why not, when it cannot.
        One that already runs is refused (N1 review: its input was not
        turned off, and it was never stopped)."""
        if self.running():
            return "a server already runs for this folder: stop it first (pocketdesk stop with its folders)"
        self.input_off()
        (self.root / "downloads").mkdir(exist_ok=True)
        (self.root / "home").mkdir(exist_ok=True)
        (self.root / "state" / "live_sessions.json").unlink(missing_ok=True)
        full = dict(os.environ)
        full.update(self.env())
        with open(self.root / "device_checks.log", "w", encoding="utf-8") as log:
            self.child = subprocess.Popen([str(self.exe), "start"], stdout=log, stderr=subprocess.STDOUT, env=full)
        # Started once it listens: its pid file comes before the bind.
        if not wait_for(lambda: "Control listener ready" in self.log() or self.child.poll() is not None, 60, 1):
            return "it did not start listening within 60 s"
        if self.child.poll() is not None:
            return "it stopped at once (see device_checks.log)"
        return None

    def stop(self) -> None:
        if self.child is None:
            return
        run([str(self.exe), "stop"], env=self.env())
        try:
            self.child.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.child.kill()

    def linked(self) -> list[str]:
        path = self.root / "state" / "live_sessions.json"
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            # Absent, or being replaced by the server just then (Windows
            # refuses the read): none this time, as `in_session` reads it.
            return []
        return fresh_linked(text, time.time())

    def event_count(self) -> int:
        path = self.root / "state" / "events.jsonl"
        return len(path.read_text(encoding="utf-8").splitlines()) if path.exists() else 0

    def events_since(self, count: int, kind: str, device: str, app: str) -> list[dict]:
        """The events of `kind` after the log's first `count` lines, from
        `device`'s `app` (N1 review: by the second, any app's matched)."""
        path = self.root / "state" / "events.jsonl"
        if not path.exists():
            return []
        lines = path.read_text(encoding="utf-8").splitlines()[count:]
        return [e for e in events_after(lines, 0, kind) if e.get("device_id") == device and e.get("app") == app]

    def events_after_count(self, count: int) -> list[dict]:
        """Every event after the log's first `count` lines."""
        path = self.root / "state" / "events.jsonl"
        if not path.exists():
            return []
        out = []
        for line in path.read_text(encoding="utf-8").splitlines()[count:]:
            try:
                out.append(json.loads(line))
            except ValueError:
                continue
        return out

    def clipboard(self) -> str:
        path = self.root / "clip.txt"
        return path.read_text(encoding="utf-8") if path.exists() else ""

    def plays(self, title: Optional[str], playing: bool = True) -> None:
        """What this computer plays, as the seam reads it (N5); None: nothing."""
        path = self.root / "computer_media.json"
        if title is None:
            path.unlink(missing_ok=True)
            return
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps({"title": title, "artist": "N5 artist", "app": "N5 player", "isPlaying": playing}), encoding="utf-8")
        os.replace(tmp, path)

    def computer_presses(self) -> list[str]:
        """The presses the seam took (N5), oldest first."""
        path = self.root / "computer_media.json.presses"
        return path.read_text(encoding="utf-8").split() if path.exists() else []

    def media(self, device: str) -> Optional[dict]:
        """`device`'s player as the server keeps it (N2), with its cover's file."""
        path = self.root / "state" / "phone_media.json"
        try:
            return json.loads(path.read_text(encoding="utf-8"))["phones"].get(device)
        except (OSError, ValueError, KeyError):
            return None

    def press_media(self, device: str, command: str) -> None:
        """As the desktop app queues a press (core::local::phone_media::press)."""
        folder = self.root / "state" / "phone_media_out"
        folder.mkdir(exist_ok=True)
        pid = str(uuid.uuid4())
        tmp = folder / f"{pid}.json.tmp"
        tmp.write_text(json.dumps({"id": pid, "device_id": device, "command": command,
                                   "created_unix": int(time.time())}), encoding="utf-8")
        os.replace(tmp, folder / f"{pid}.json")
    def note(self, device: str, title: str) -> Optional[dict]:
        """The phone notification the server keeps to answer, by its title."""
        path = self.root / "state" / "phone_notifications.json"
        try:
            notes = json.loads(path.read_text(encoding="utf-8")).get("notes", [])
        except (OSError, ValueError):
            return None
        return next((n for n in notes if n.get("device_id") == device and n.get("title") == title), None)

    def open_note(self, device: str, key: str) -> str:
        """Ask the phone to open `key`, as the Notifications page does; the request's id."""
        folder = self.root / "state" / "notification_out"
        folder.mkdir(exist_ok=True)
        rid = str(uuid.uuid4())
        tmp = folder / f"{rid}.json.tmp"
        tmp.write_text(json.dumps({"id": rid, "device_id": device, "key": key, "answer": "open",
                                   "created_unix": int(time.time())}), encoding="utf-8")
        os.replace(tmp, folder / f"{rid}.json")
        return rid

    def card(self, device: str, title: str) -> Optional[dict]:
        """A phone's live card the server keeps (N4), by its title."""
        path = self.root / "state" / "phone_live.json"
        try:
            cards = json.loads(path.read_text(encoding="utf-8")).get("cards", [])
        except (OSError, ValueError):
            return None
        return next((c for c in cards if c.get("device_id") == device and c.get("title") == title), None)

    def answer_state(self, rid: str) -> Optional[dict]:
        path = self.root / "state" / "notification_answers.json"
        try:
            answers = json.loads(path.read_text(encoding="utf-8")).get("answers", [])
        except (OSError, ValueError):
            return None
        return next((a for a in answers if a.get("id") == rid), None)

    def type_batch(self, device: str, seq: int, events: list[dict]) -> None:
        """As the desktop's typing window writes it (core::local::typing)."""
        folder = self.root / "state" / "typing_out"
        folder.mkdir(exist_ok=True)
        ms = int(time.time() * 1000)
        name = f"{ms:013}-{seq:010}.json"
        body = {"device_id": device, "seq": seq, "created_ms": ms, "batches": [events]}
        (folder / (name + ".tmp")).write_text(json.dumps(body), encoding="utf-8")
        os.replace(folder / (name + ".tmp"), folder / name)


# ---- the checks ----


@dataclass
class Ctx:
    server: Server
    phone: Phone
    device: str
    # The phone's saved control port and the throwaway's, for `adb reverse`.
    phone_port: int = 1716
    port: int = 1726
    # The session checks moved the phone to the internet road (W1).
    on_road: bool = False


Check = Callable[[Ctx], tuple[str, str]]
CHECKS: dict[str, tuple[str, Check]] = {}


def check(cid: str, section: str):
    def register(fn: Check) -> Check:
        CHECKS[cid] = (section, fn)
        return fn
    return register


def needs(ctx: Ctx, pref: str) -> Optional[str]:
    if not ctx.phone.prefs("pocketdesk-ui").get(pref):
        return f"turn on {pref} in the phone's Settings"
    if not ctx.phone.accessibility_on():
        return "turn on Pocketdesk control in Android's Accessibility settings"
    return None


@check("m4-typing", "§171")
def m4_typing(ctx: Ctx) -> tuple[str, str]:
    if (why := needs(ctx, "typing_from_computer")):
        return "SKIP", why
    ctx.phone.wake()
    if not ctx.phone.field(""):
        return "TIMEOUT", "the fixture's field never came up"
    return typed(ctx, 1, [{"text": "Héllo தமிழ்"}, {"key": "backspace"}, {"text": "!"}], "Héllo தமிழ!")


def typed(ctx: Ctx, seq: int, events: list[dict], want: str) -> tuple[str, str]:
    """Type `events` once the control service is back, and read the field."""
    if not ctx.phone.service_back():
        return "TIMEOUT", "Pocketdesk control did not come back after the screen was read"
    ctx.server.type_batch(ctx.device, seq, events)
    # Read once the batch has had time: each read takes the service away.
    time.sleep(3)
    got = wait_for(lambda: field_text(ctx.phone.dump()) == want, 15, step=3)
    return ("PASS", f"read back {want!r}") if got else ("TIMEOUT", f"field holds {field_text(ctx.phone.dump())!r}")


@check("m4-locked", "§171")
def m4_locked(ctx: Ctx) -> tuple[str, str]:
    if (why := needs(ctx, "typing_from_computer")):
        return "SKIP", why
    ctx.phone.wake()
    if not ctx.phone.field("keep"):
        return "TIMEOUT", "the fixture's field never came up"
    if not ctx.phone.service_back():
        return "TIMEOUT", "Pocketdesk control did not come back after the screen was read"
    ctx.phone.sleep()
    ctx.server.type_batch(ctx.device, 2, [{"text": " LOCKED"}])
    time.sleep(6)
    ctx.phone.wake()
    got = field_text(ctx.phone.dump())
    if got != "keep":
        return "FAIL", f"field holds {got!r}"
    # The control: typing reaches it once unlocked, so the batch was refused,
    # not lost on the way (N1 review).
    status, detail = typed(ctx, 3, [{"text": " ok"}], "keep ok")
    return ("PASS", "nothing typed while locked; typed once unlocked") if status == "PASS" else (status, "control: " + detail)


def copies_ready(ctx: Ctx) -> Optional[str]:
    if ctx.phone.sdk() < 33:
        return "Android 13 or later"
    if (why := needs(ctx, "copies_by_themselves")):
        return why
    return None


def copied(ctx: Ctx, sensitive: bool = False) -> tuple[str, bool]:
    """Copy a fresh token once the control service is back; whether it reached the computer."""
    ctx.phone.wake()
    ctx.phone.service_back(timeout=5)
    token = ("secret-" if sensitive else "copy-") + secrets.token_hex(4)
    ctx.phone.copy(token, sensitive)
    return token, bool(wait_for(lambda: ctx.server.clipboard() == token, 8 if sensitive else 20))


@check("m10-copy", "§175")
def m10_copy(ctx: Ctx) -> tuple[str, str]:
    if (why := copies_ready(ctx)):
        return "SKIP", why
    token, arrived = copied(ctx)
    return ("PASS", f"{token} reached the computer") if arrived else ("TIMEOUT", f"clipboard holds {ctx.server.clipboard()!r}")


@check("m10-sensitive", "§175")
def m10_sensitive(ctx: Ctx) -> tuple[str, str]:
    if (why := copies_ready(ctx)):
        return "SKIP", why
    _, arrived = copied(ctx, sensitive=True)
    if arrived:
        return "FAIL", "a sensitive copy reached the computer"
    # The control: a plain copy right after does arrive (N1 review).
    token, control = copied(ctx)
    return ("PASS", "not sent; a plain copy after it was") if control else ("TIMEOUT", "control: the plain copy did not arrive either")


def forwarding(ctx: Ctx) -> Optional[str]:
    notes = ctx.phone.prefs("pocketdesk-phone-notifications")
    if not notes.get("enabled") or FIXTURE not in (notes.get("shared") or []):
        return f"turn on This phone's notifications for {FIXTURE}"
    if not ctx.phone.prefs("pocketdesk-ui").get("quiet_in_hand"):
        return "turn on Quiet on the computer while I use the phone"
    if not ctx.phone.may_notify(FIXTURE):
        return "allow the fixture's notifications (open its field once and press Allow)"
    return None


def fixture_note(ctx: Ctx, nid: int, text: str, timeout: float) -> Optional[dict]:
    """Post a fixture notification; its event on the computer, if it came."""
    count = ctx.server.event_count()
    ctx.phone.notify(nid, "Ann", text)
    got = wait_for(lambda: ctx.server.events_since(count, "notification", ctx.device, "Pocketdesk fixture"), timeout)
    return got[-1] if got else None


@check("m6-in-hand", "§170")
def m6_in_hand(ctx: Ctx) -> tuple[str, str]:
    if (why := forwarding(ctx)):
        return "SKIP", why
    ctx.phone.wake()
    event = fixture_note(ctx, 601, "In hand " + secrets.token_hex(2), 25)
    if not event:
        return "TIMEOUT", "no notification reached the computer"
    return ("PASS", "held: phone_in_use") if event.get("phone_in_use") else ("FAIL", "shown while in hand")


@check("m6-locked", "§170")
def m6_locked(ctx: Ctx) -> tuple[str, str]:
    if (why := forwarding(ctx)):
        return "SKIP", why
    ctx.phone.sleep()
    event = fixture_note(ctx, 602, "Locked " + secrets.token_hex(2), 30)
    ctx.phone.wake()
    if not event:
        return "TIMEOUT", "no notification reached the computer"
    return ("FAIL", "held while locked") if event.get("phone_in_use") else ("PASS", "shown")


@check("m8-tel", "§173")
def m8_tel(ctx: Ctx) -> tuple[str, str]:
    adb("logcat", "-c")
    code, out = run([str(ctx.server.exe), "tel", "tel:+1-555-0100"], env=ctx.server.env())
    if code != 0 or "Calling +15550100" not in out:
        return "FAIL", out.strip().splitlines()[-1] if out.strip() else f"exit {code}"
    # What the phone made of it, as its own log says (N1 review: a refusal
    # had passed as "reached").
    seen = wait_for(lambda: re.search(r"a number from the computer (was not taken: \S+|was \w+)", ctx.phone.logcat("Dials")), 30)
    if not seen:
        return "TIMEOUT", "the dial did not reach the phone"
    outcome = seen.group(1)
    on = ctx.phone.prefs("pocketdesk-ui").get("calls_from_computer")
    if on and "not taken" in outcome:
        return "FAIL", f"chose the linked phone, which {outcome} with Calls from the computer on"
    return "PASS", f"chose the linked phone, whose answer was: {outcome}"


@check("n2-media", "§176")
def n2_media(ctx: Ctx) -> tuple[str, str]:
    ui = ctx.phone.prefs("pocketdesk-ui")
    if not (ui.get("phone_media_on_computer") and ui.get("phone_media_background")):
        return "SKIP", "turn on My media on the computer in the background in the phone's Settings"
    title = "N2 check " + secrets.token_hex(3)
    try:
        ctx.phone.play(title)
        # No session: the listener's report, with its cover.
        got = wait_for(lambda: (m := ctx.server.media(ctx.device)) and m["media"].get("title") == title and m, 20)
        if not got:
            return "TIMEOUT", "the song did not reach the computer"
        media = got["media"]
        if not media.get("background") or not got.get("art"):
            return "FAIL", f"background {media.get('background')}, cover {got.get('art')}"
        cover = ctx.server.root / "state" / "phone_media_art" / got["art"]
        if not cover.is_file() or cover.read_bytes()[:2] != bytes([0xFF, 0xD8]):
            return "FAIL", "no cover kept"
        ctx.server.press_media(ctx.device, "playPause")
        paused = wait_for(lambda: (m := ctx.server.media(ctx.device)) and not m["media"]["isPlaying"], 20)
        if not paused:
            return "TIMEOUT", "Pause from the computer did not reach the player"
    finally:
        ctx.phone.stop_playing()
    gone = wait_for(lambda: ctx.server.media(ctx.device) is None, 20)
    return ("PASS", "shown with its cover, paused from here, gone when it ended") if gone else (
        "TIMEOUT", "an ended player stayed on the computer")
def openable(ctx: Ctx, title: str) -> tuple[Optional[str], Optional[dict]]:
    """Post a fixture notification and wait for the computer to keep it as openable."""
    notes = ctx.phone.prefs("pocketdesk-phone-notifications")
    if not notes.get("enabled") or FIXTURE not in (notes.get("shared") or []):
        return f"turn on This phone's notifications for {FIXTURE}", None
    if not ctx.phone.prefs("pocketdesk-ui").get("let_computer_open"):
        return "turn on Let the computer open my notifications", None
    ctx.phone.notify(int(time.time()) % 100000, title, "Open me")
    note = wait_for(lambda: (n := ctx.server.note(ctx.device, title)) and n.get("open") and n, 25)
    return None, note


@check("n3-open", "§177")
def n3_open(ctx: Ctx) -> tuple[str, str]:
    ctx.phone.wake()
    title = "N3 open " + secrets.token_hex(2)
    why, note = openable(ctx, title)
    if why:
        return "SKIP", why
    if not note:
        return "TIMEOUT", "the computer never kept it as openable"
    if not ctx.phone.accessibility_on():
        return "SKIP", "turn on Pocketdesk control in Android's Accessibility settings"
    adb("shell", "input", "keyevent", "KEYCODE_HOME")
    rid = ctx.server.open_note(ctx.device, note["key"])
    done = wait_for(lambda: (a := ctx.server.answer_state(rid)) and a["state"]["state"] in ("handed", "not_sent") and a, 30)
    if not done:
        return "TIMEOUT", "no word on the open"
    if done["state"]["state"] != "handed":
        return "FAIL", done["state"].get("reason", "not sent")
    # The activity draws a moment after the word comes (N3 review).
    opened = wait_for(lambda: (t := field_text(ctx.phone.dump()) or "").startswith("opened ") and t, 10, step=2)
    said = done.get("said", "")
    return ("PASS", f"{said}: {opened!r}") if opened else (
        "FAIL", f"{said or 'handed'}, but the screen shows {field_text(ctx.phone.dump())!r}")


@check("n3-locked", "§177")
def n3_locked(ctx: Ctx) -> tuple[str, str]:
    ctx.phone.wake()
    title = "N3 locked " + secrets.token_hex(2)
    why, note = openable(ctx, title)
    if why:
        return "SKIP", why
    if not note:
        return "TIMEOUT", "the computer never kept it as openable"
    ctx.phone.sleep()
    rid = ctx.server.open_note(ctx.device, note["key"])
    done = wait_for(lambda: (a := ctx.server.answer_state(rid)) and a["state"]["state"] in ("handed", "not_sent") and a, 30)
    ctx.phone.wake()
    if not done:
        return "TIMEOUT", "no word on the open"
    reason = done["state"].get("reason", "")
    return ("PASS", f"refused: {reason}") if done["state"]["state"] == "not_sent" and "locked" in reason else (
        "FAIL", f"{done['state']['state']} {reason}")


@check("n4-live", "§178")
def n4_live(ctx: Ctx) -> tuple[str, str]:
    notes = ctx.phone.prefs("pocketdesk-phone-notifications")
    if not notes.get("enabled") or FIXTURE not in (notes.get("shared") or []):
        return "SKIP", f"turn on This phone's notifications for {FIXTURE}"
    if not ctx.phone.prefs("pocketdesk-ui").get("live_updates"):
        return "SKIP", "turn on Ongoing updates on the computer"
    ctx.phone.wake()
    nid = int(time.time()) % 100000
    title = "N4 live " + secrets.token_hex(2)
    progress = lambda: (c := ctx.server.card(ctx.device, title)) and c["live"]["progress"]
    try:
        for step in (10, 50, 52):
            ctx.phone.notify(nid, title, "file.bin", style="progress", progress=step)
            time.sleep(1)
        # 50 and 52 come within the gap after 10: held back, the last is
        # looked at again once the gap passes (N4 review: 100 never waits).
        if not wait_for(lambda: progress() == 52, 15):
            return "TIMEOUT", f"the held-back state did not arrive: {progress()!r}"
        ctx.phone.notify(nid, title, "file.bin", style="progress", progress=100)
        if not wait_for(lambda: progress() == 100, 15):
            return "TIMEOUT", f"the card stopped at {progress()!r}"
    finally:
        ctx.phone.cancel(nid)
    gone = wait_for(lambda: ctx.server.card(ctx.device, title) is None, 20)
    return ("PASS", "the held-back 52 % came, then 100 %, and the card went with its end") if gone else (
        "TIMEOUT", "the card stayed after its end")


@check("n5-media", "§179")
def n5_media(ctx: Ctx) -> tuple[str, str]:
    if not ctx.phone.prefs("pocketdesk-ui").get("computer_media_controls"):
        return "SKIP", "turn on Computer's music in my media controls in the phone's Settings"
    title = "N5 check " + secrets.token_hex(3)
    try:
        ctx.server.plays(title)
        if not wait_for(lambda: ctx.phone.shows_media(title), 30, step=2):
            return "TIMEOUT", "the computer's song never showed in the phone's media session"
        # Never sent back as the phone's own music (N2's reporter is on here).
        time.sleep(4)
        echo = ctx.server.media(ctx.device)
        if echo and echo["media"].get("title") == title:
            return "FAIL", "the computer's song came back as the phone's music"
        # A media key goes to the session playing: the computer's, here.
        before = len(ctx.server.computer_presses())
        adb("shell", "cmd", "media_session", "dispatch", "next")
        pressed = wait_for(lambda: ctx.server.computer_presses()[before:], 15)
        if not pressed:
            return "FAIL", "shown, but a media key did not reach the computer (another player may hold the keys)"
        if pressed[0] != "next":
            return "FAIL", f"shown, but the press came as {pressed[0]!r}"
    finally:
        ctx.server.plays(None)
    gone = wait_for(lambda: not ctx.phone.shows_media(title), 20, step=2)
    return ("PASS", "shown in the phone's media controls, Next reached the computer, gone when it stopped") if gone else (
        "TIMEOUT", "the session stayed after the computer stopped playing")


@check("n6-put", "§180")
def n6_put(ctx: Ctx) -> tuple[str, str]:
    from urllib.parse import quote
    drive = ctx.server.root / "state" / "drive.json"
    if not drive.exists():
        return "SKIP", "set phone_drive = true in the throwaway server's config.toml"
    info = json.loads(drive.read_text(encoding="utf-8"))
    port, token = info["port"], info["token"]
    folders = ctx.phone.prefs("pocketdesk-phone-folders").get("folders")
    marked, unmarked = marked_folder(folders if isinstance(folders, str) else "[]")
    if not marked:
        return "SKIP", "share a folder on the phone and turn on its \"The computer may add files\""
    # The phone on the drive whose folders show the marked one (N6 review:
    # not merely the first paired phone).
    status, top = dav(port, "PROPFIND", f"/{token}/", depth="1")
    phones = dav_children(top.decode("utf-8", "replace"), f"/{token}/")
    if status != 207 or not phones:
        return "FAIL", f"the drive lists no phone ({status})"
    found = None
    for phone in phones:
        status, listing = dav(port, "PROPFIND", f"/{token}/{quote(phone)}/", depth="1")
        roots = dav_children(listing.decode("utf-8", "replace"), f"/{token}/{phone}/") if status == 207 else []
        if drive_root(roots, marked):
            found = (phone, drive_root(roots, marked), drive_root(roots, unmarked))
            break
    if not found:
        return "FAIL", f"no phone on the drive shows {marked!r}: is the phone linked?"
    phone, marked, unmarked = found
    base = f"/{token}/{quote(phone)}"
    folder = "N6 check " + secrets.token_hex(3)
    made = dav(port, "MKCOL", f"{base}/{quote(marked)}/{quote(folder)}")[0]
    if made != 201:
        return "FAIL", f"MKCOL answered {made}"
    # The new folder in its folder's next listing, from the phone.
    def listed():
        status, listing = dav(port, "PROPFIND", f"{base}/{quote(marked)}/", depth="1")
        return status == 207 and folder in dav_children(listing.decode("utf-8", "replace"), f"/{token}/{phone}/{marked}/")
    if not wait_for(listed, 20, step=2):
        return "TIMEOUT", "the new folder did not show on the phone"
    if unmarked:
        refused = dav(port, "PUT", f"{base}/{quote(unmarked)}/n6.bin", b"x")[0]
        if refused != 403:
            return "FAIL", f"a PUT into an unmarked folder answered {refused}"
    # A file's content is not put here: through adb reverse the phone is on
    # loopback, which transfers never dial back, so it would fail (and tell
    # this computer's owner so). Its write is checked on the LAN road.
    refused = ", refused in an unmarked folder" if unmarked else ""
    return "PASS", f"made a folder the phone listed after{refused}; a file's write is owed on the LAN road"


# ---- the O and P cycles (Q1) ----


def sharing(ctx: Ctx) -> Optional[str]:
    """Why the fixture's notifications would not reach the computer, if not."""
    notes = ctx.phone.prefs("pocketdesk-phone-notifications")
    if not notes.get("enabled") or FIXTURE not in (notes.get("shared") or []):
        return f"turn on This phone's notifications for {FIXTURE}"
    if not ctx.phone.may_notify(FIXTURE):
        return "allow the fixture's notifications (open its field once and press Allow)"
    return None


def asleep_note(ctx: Ctx, nid: int, text: str, style: str = "plain", timeout: float = 30) -> Optional[dict]:
    """A fixture notification posted with the phone asleep, so M6's hold
    for a phone in hand never comes first; its event on the computer."""
    count = ctx.server.event_count()
    ctx.phone.sleep()
    ctx.phone.notify(nid, "Ann", text, style=style)
    got = wait_for(lambda: ctx.server.events_since(count, "notification", ctx.device, "Pocketdesk fixture"), timeout)
    ctx.phone.cancel(nid)
    ctx.phone.wake()
    return got[-1] if got else None


def clock_at(minutes_from_now: int) -> str:
    """This computer's local time `minutes_from_now`, as `HH:MM`."""
    return time.strftime("%H:%M", time.localtime(time.time() + 60 * minutes_from_now))


def front_activity() -> str:
    """The activity in front, as `dumpsys activity` names it."""
    found = re.search(r"(?:topResumedActivity|mResumedActivity)=\S+ \S+ (\S+)", adb("shell", "dumpsys", "activity", "activities"))
    return found.group(1) if found else ""


ALL_DAYS = '["mon", "tue", "wed", "thu", "fri", "sat", "sun"]'


@check("p2-apps", "§194")
def p2_apps(ctx: Ctx) -> tuple[str, str]:
    if not ctx.phone.prefs("pocketdesk-ui").get("apps_from_computer"):
        return "SKIP", "turn on Let the computer open my apps in the phone's Settings"
    ctx.phone.wake()
    asked = int(time.time())
    ctx.server.ask_phone(ctx.device, "apps")
    listed = wait_for(lambda: (k := ctx.server.kept("phone_apps.json", ctx.device)) and k["at"] >= asked and k, 30)
    try:
        if not listed:
            return "TIMEOUT", "no list of apps came"
        ids = [a.get("id") for a in listed["apps"]]
        if APP in ids:
            return "FAIL", "Pocketdesk listed itself"
        if "com.android.settings" not in ids:
            return "FAIL", f"{len(ids)} apps listed, Settings not among them"
        adb("shell", "input", "keyevent", "KEYCODE_HOME")
        adb("logcat", "-c")
        ctx.server.ask_phone(ctx.device, "open", "com.android.settings")
        # What the phone made of it, as its own log says (Q1 review: any
        # "Tap to open" left by an earlier check had passed): at once with
        # the control service bound, else offered with "Tap to open".
        said = wait_for(lambda: re.search(r"an app the computer named: ([^\r\n]+)", ctx.phone.logcat("PhoneActions")), 20)
        front = said and said.group(1) == "opened" and wait_for(lambda: "com.android.settings" in front_activity(), 10)
    finally:
        (ctx.server.root / "state" / "phone_apps.json").unlink(missing_ok=True)
        adb("shell", "input", "keyevent", "KEYCODE_HOME")
    if not said:
        return "TIMEOUT", f"{len(ids)} apps listed, but no word on the open"
    outcome = said.group(1)
    if outcome == "opened":
        return ("PASS", f"{len(ids)} apps listed; Settings opened") if front else (
            "FAIL", "the phone said opened, but Settings is not in front")
    if outcome == "offered" and not ctx.phone.accessibility_on():
        return "PASS", f"{len(ids)} apps listed; Settings offered with Tap to open (Pocketdesk control off)"
    return "FAIL", f"the open was {outcome!r}"


@check("p3-presenting", "§195")
def p3_presenting(ctx: Ctx) -> tuple[str, str]:
    if (why := sharing(ctx)):
        return "SKIP", why
    flag = ctx.server.root / "presenting"
    before = ctx.server.configure({"quiet_presenting": "true", "quiet_hours": "false"})
    try:
        flag.write_text("", encoding="utf-8")
        held = asleep_note(ctx, 701, "Presenting " + secrets.token_hex(2))
        flag.unlink()
        shown = asleep_note(ctx, 702, "Not presenting " + secrets.token_hex(2))
    finally:
        flag.unlink(missing_ok=True)
        ctx.server.restore_config(before)
    if not held or not shown:
        return "TIMEOUT", "a notification did not reach the computer"
    if not held.get("presenting"):
        return "FAIL", "shown while presenting"
    if shown.get("presenting") or shown.get("alarm") or held.get("alarm"):
        return "FAIL", "held with nothing presented, or marked alarm"
    return "PASS", "held while presenting (presenting); shown after"


@check("p3-hours", "§195")
def p3_hours(ctx: Ctx) -> tuple[str, str]:
    if (why := sharing(ctx)):
        return "SKIP", why
    common = {"quiet_hours": "true", "quiet_presenting": "false", "quiet_days": ALL_DAYS}
    before = ctx.server.configure({**common, "quiet_from": f'"{clock_at(-2)}"', "quiet_to": f'"{clock_at(10)}"'})
    try:
        held = asleep_note(ctx, 703, "Quiet now " + secrets.token_hex(2))
        ctx.server.configure({**common, "quiet_from": f'"{clock_at(60)}"', "quiet_to": f'"{clock_at(70)}"'})
        shown = asleep_note(ctx, 704, "Quiet later " + secrets.token_hex(2))
    finally:
        ctx.server.restore_config(before)
    if not held or not shown:
        return "TIMEOUT", "a notification did not reach the computer"
    if not held.get("quiet_hours"):
        return "FAIL", "shown within quiet hours"
    if shown.get("quiet_hours") or shown.get("alarm") or held.get("alarm"):
        return "FAIL", "held by a span later today, or marked alarm"
    return "PASS", "held within quiet hours (quiet_hours); shown with a span later today"


@check("p4-words", "§196")
def p4_words(ctx: Ctx) -> tuple[str, str]:
    if (why := sharing(ctx)):
        return "SKIP", why
    words = "p4 " + secrets.token_hex(3)
    before = ctx.server.configure({"hold_words": json.dumps([words])})
    try:
        held = asleep_note(ctx, 705, f"Today only: {words.upper()}")
        shown = asleep_note(ctx, 706, "Lunch? " + secrets.token_hex(2))
    finally:
        ctx.server.restore_config(before)
    if not held or not shown:
        return "TIMEOUT", "a notification did not reach the computer"
    if not held.get("held_by_words"):
        return "FAIL", "shown with the words in it"
    if shown.get("held_by_words") or shown.get("alarm") or held.get("alarm"):
        return "FAIL", "held without the words, or marked alarm"
    return "PASS", "held by its words, case aside (held_by_words); another shown"


@check("p5-alarm", "§197")
def p5_alarm(ctx: Ctx) -> tuple[str, str]:
    if not ctx.phone.prefs("pocketdesk-ui").get("alarms_on_computer"):
        return "SKIP", "turn on Alarms on the computer in the phone's Settings"
    if (why := sharing(ctx)):
        return "SKIP", why
    # Paused, so nothing rings on this computer.
    code, out = ctx.server.cli("pause", "--for", "5")
    if code != 0:
        return "FAIL", f"the pause: {out.strip()[-200:]}"
    try:
        event = asleep_note(ctx, 707, "Wake up " + secrets.token_hex(2), style="alarm")
        plain = asleep_note(ctx, 708, "Not an alarm " + secrets.token_hex(2))
    finally:
        # 4. A pause left on would hold the next run's notifications.
        resumed, out = ctx.server.cli("pause", "--resume")
    if resumed != 0:
        return "FAIL", f"the pause was not ended: {out.strip()[-200:]}"
    if not event or not plain:
        return "TIMEOUT", "a notification did not reach the computer"
    if not event.get("alarm"):
        return "FAIL", "the alarm reached the computer as a plain notification"
    if plain.get("alarm"):
        return "FAIL", "a plain notification was marked alarm"
    return "PASS", "marked alarm, a plain one not (paused here, so it did not ring)"


@check("p6-several", "§198")
def p6_several(ctx: Ctx) -> tuple[str, str]:
    path = ctx.server.root / "p6-check.txt"
    path.write_text("P6 " + secrets.token_hex(4), encoding="utf-8")
    try:
        code, out = ctx.server.cli("send", f"{ctx.device},nosuchphone,{ctx.device}", str(path), "--json")
    finally:
        path.unlink(missing_ok=True)
    result = next((json.loads(line) for line in reversed(out.splitlines()) if line.startswith("{")), None)
    if not result:
        return "FAIL", f"no JSON (exit {code})"
    sends = result.get("sends") or []
    if len(sends) != 2:
        return "FAIL", f"{len(sends)} sends, not 2 (the repeat skipped, the unknown refused)"
    if result.get("error") != "1 of 2 did not go" or code != 2:
        return "FAIL", f"error {result.get('error')!r}, exit {code}"
    return "PASS", "the phone once, the repeat skipped, the unknown refused: 1 of 2 did not go, exit 2"


@check("p7-files", "§199")
def p7_files(ctx: Ctx) -> tuple[str, str]:
    files = [ctx.server.root / f"p7-{n}.txt" for n in (1, 2)]
    for f in files:
        f.write_text("P7", encoding="utf-8")
    listing = ctx.server.root / "clip_files.txt"
    listing.write_text("".join(f"{f}\n" for f in files), encoding="utf-8")
    seen = len(ctx.server.log())
    try:
        ctx.server.send_clipboard(ctx.device)
        said = wait_for(lambda: re.search(r"(Queued \d+ copied file\(s\)|not sent the copied files: [^\n]*|not sent the copy[^\n]*)",
                                          ctx.server.log()[seen:]), 15)
    finally:
        listing.unlink(missing_ok=True)
        # A queued offer is read when the phone fetches it, which over adb
        # reverse it cannot: the files go now.
        for f in files:
            f.unlink(missing_ok=True)
    if not said:
        return "TIMEOUT", "the request was not taken"
    line = said.group(1)
    if line.startswith("not sent the copy:"):
        return "FAIL", f"taken as text: {line}"
    # Over adb reverse files cannot reach the phone, so "phone not connected"
    # is the answer expected here; on the LAN they are queued.
    return "PASS", f"read as files: {line}"


@check("p10-contacts", "§203")
def p10_contacts(ctx: Ctx) -> tuple[str, str]:
    if not ctx.phone.prefs("pocketdesk-texts").get("contacts_to_computer"):
        return "SKIP", "turn on My contacts on the computer in the phone's Settings"
    if not ctx.phone.granted("android.permission.READ_CONTACTS"):
        return "SKIP", "allow Pocketdesk's contacts permission"
    ctx.phone.wake()
    asked = int(time.time())
    ctx.server.ask_phone(ctx.device, "contacts")
    try:
        # Counts only: the list is the phone owner's.
        listed = wait_for(lambda: (k := ctx.server.kept("phone_contacts.json", ctx.device)) and k["at"] >= asked and k, 30)
        count = len(listed["contacts"]) if listed else 0
    finally:
        (ctx.server.root / "state" / "phone_contacts.json").unlink(missing_ok=True)
    if not listed:
        return "TIMEOUT", "no list of contacts came"
    if count == 0:
        return "SKIP", "the phone listed no contacts: add one to check the list"
    return "PASS", f"{count} contacts listed (counts only; the list deleted)"


@check("p11-ring", "§202")
def p11_ring(ctx: Ctx) -> tuple[str, str]:
    ui = ctx.phone.prefs("pocketdesk-ui")
    if not ui.get("ring_from_computer"):
        return "SKIP", "turn on Ring from the computer in the phone's Settings"
    vibrates, flashes = ui.get("ring_vibrates", True), ui.get("ring_flashes", True)
    if not (vibrates or flashes):
        return "SKIP", "turn on Vibrate or Flash the torch while it rings"
    since = ctx.phone.clock()
    code, out = ctx.server.cli("ring", ctx.device)
    if code != 0:
        return "FAIL", out.strip().splitlines()[-1] if out.strip() else f"exit {code}"
    # A vibration is listed once it has ended: the ring's 30 s.
    def shaken():
        return [v for v in alarm_vibrations(adb("shell", "dumpsys", "vibrator_manager"), APP) if v[0] >= since]
    if vibrates:
        shook = wait_for(shaken, 50, step=2)
    else:
        time.sleep(35)
        shook = shaken()
    turns = torch_turns(adb("shell", "dumpsys", "media.camera"), since)
    said = []
    if vibrates:
        if not shook:
            return "TIMEOUT", "no vibration ended within 50 s"
        if shook[-1][1] < 25000:
            return "FAIL", f"vibrated {shook[-1][1]} ms, not the ring's 30 s"
        said.append(f"vibrated {shook[-1][1]} ms")
    if flashes:
        blinks = sum(1 for _, how in turns if how == "on")
        if blinks < 3:
            return "FAIL", f"the torch turned on {blinks} times"
        if turns[-1][1] != "off":
            return "FAIL", "the torch was left on"
        said.append(f"the torch blinked {blinks} times, off at the end")
    if not vibrates and shook:
        return "FAIL", "vibrated with Vibrate off"
    if not flashes and turns:
        return "FAIL", "the torch blinked with Flash off"
    return "PASS", ", ".join(said)


# ---- the O cycle (R1) ----

# A task that only echoes, and a deck button that runs it: a key, text,
# media or link button would act on this computer, so none is used.
R1_TASK = "R1 echo"
R1_DECK = "R1 deck"
R1_CONFIG = f"""
[[tasks]]
name = "{R1_TASK}"
command = "echo r1"
ask = false

[[deck]]
label = "{R1_DECK}"
task = "{R1_TASK}"
"""


def automation(action: str, name: str) -> None:
    """O1's broadcast, as an automation app sends it, for the throwaway
    saved at 127.0.0.1."""
    adb("logcat", "-c")
    am("broadcast", "-n", f"{APP}/.automation.AutomationReceiver", "-a", f"{APP}.action.{action}",
       "--es", f"{APP}.extra.ADDRESS", "127.0.0.1", "--es", f"{APP}.extra.NAME", name)


@check("o1-task", "§184")
def o1_task(ctx: Ctx) -> tuple[str, str]:
    if not ctx.phone.prefs("pocketdesk-ui").get("others_run_tasks"):
        return "SKIP", "turn on Let other apps run the computer's tasks in the phone's Settings"
    before = ctx.server.append_config(R1_CONFIG)
    try:
        count = ctx.server.event_count()
        # The control first: a name the computer has none of runs nothing.
        automation("RUN_TASK", "R1 no such task")
        unknown = wait_for(lambda: "no task by that name" in ctx.phone.logcat("Automation"), 30)
        automation("RUN_TASK", R1_TASK)
        done = wait_for(lambda: [e for e in ctx.server.events_of(count, "task_finished", ctx.device)
                                 if e.get("task") == R1_TASK], 45)
    finally:
        ctx.server.restore_config(before)
    if not unknown:
        return "TIMEOUT", "no answer for a task the computer does not have"
    if not done:
        return "TIMEOUT", "the task did not finish on the computer"
    code = done[-1].get("code")
    return ("PASS", "ran on the computer, code 0; an unknown name refused") if code == 0 else (
        "FAIL", f"the task ended with code {code}")


@check("o1-deck", "§184")
def o1_deck(ctx: Ctx) -> tuple[str, str]:
    if not ctx.phone.prefs("pocketdesk-ui").get("others_run_tasks"):
        return "SKIP", "turn on Let other apps run the computer's tasks in the phone's Settings"
    before = ctx.server.append_config(R1_CONFIG)
    try:
        count = ctx.server.event_count()
        automation("PRESS_DECK", R1_DECK)
        pressed = wait_for(lambda: ctx.server.events_of(count, "deck_pressed", ctx.device), 45)
        ran = wait_for(lambda: [e for e in ctx.server.events_of(count, "task_finished", ctx.device)
                                if e.get("task") == R1_TASK], 30)
    finally:
        ctx.server.restore_config(before)
    if not pressed:
        return "TIMEOUT", "no press reached the computer"
    press = pressed[-1]
    if press.get("kind") != "task" or not press.get("ok"):
        return "FAIL", f"pressed as {press.get('kind')}, ok {press.get('ok')}"
    return ("PASS", "the task button pressed, its task ran") if ran else (
        "TIMEOUT", "pressed, but its task did not finish")


R1_KEPT = "r1-kept"


@check("o5-kinds", "§187")
def o5_kinds(ctx: Ctx) -> tuple[str, str]:
    if (why := sharing(ctx)):
        return "SKIP", why
    kept = ctx.phone.prefs("pocketdesk-phone-notifications").get(f"kinds_kept:{FIXTURE}") or []
    if R1_KEPT not in kept:
        return "SKIP", f"post a fixture notification on the channel {R1_KEPT!r} once, then switch that kind off under the fixture in This phone's notifications"
    # One at a time, so each event is known (R1 review: two of one app read
    # the same, and a leaked kept one could stand in for a lost shared one).
    count = ctx.server.event_count()
    ctx.phone.sleep()
    adb("logcat", "-c")
    am("broadcast", "-n", f"{FIXTURE}/.NotifyReceiver", "-a", "dev.pocketdesk.fixture.NOTIFY",
       "--ei", "id", "709", "--es", "title", "Kept kind", "--es", "text", "R1 " + secrets.token_hex(2),
       "--es", "channel", R1_KEPT)
    posted = wait_for(lambda: "posted 709" in ctx.phone.logcat("Fixture"), 10)
    time.sleep(8)
    leaked = ctx.server.events_since(count, "notification", ctx.device, "Pocketdesk fixture")
    ctx.phone.cancel(709)
    # The control: a shared kind after it arrives, alone.
    after = ctx.server.event_count()
    ctx.phone.notify(710, "Ann", "R1 shared " + secrets.token_hex(2))
    shared = wait_for(lambda: ctx.server.events_since(after, "notification", ctx.device, "Pocketdesk fixture"), 30)
    ctx.phone.cancel(710)
    ctx.phone.wake()
    if not posted:
        return "FAIL", "the kept kind's notification was never posted on the phone"
    if leaked:
        return "FAIL", "the kept kind reached the computer"
    if not shared:
        return "TIMEOUT", "the shared kind did not reach the computer either"
    return "PASS", "the kept kind stayed on the phone; the other kind arrived"


# ---- sessions over the internet road (W1) ----
#
# A session shows this computer's screen on the phone. It runs only with the
# phone's input off in the throwaway's trust store (the server turns it off
# at each start; checked again here). The video cannot ride `adb reverse`
# (UDP), so a session goes by the internet road: the server is restarted
# without the reverse, and the phone relinks that way. Pocketdesk is never
# force-stopped (see the module's doc).

# Checks that run only when named with --only: they take long.
LONG: set[str] = set()


def input_off_for(ctx: Ctx) -> bool:
    store = json.loads((ctx.server.root / "state" / "trusted_devices.json").read_text(encoding="utf-8"))
    return store.get("permissions", {}).get(ctx.device, {}).get("input") is False


def iroh_loops(log: str) -> int:
    """Control links the server's log says came by the internet road (its
    colour codes stripped: they split `transport="iroh"`)."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    return sum('transport="iroh"' in line for line in plain.splitlines() if "Entering main packet loop" in line)


def by_road(ctx: Ctx) -> Optional[str]:
    """The server restarted without the reverse, the phone linked again by
    the internet road; why not, when it is not."""
    ctx.server.stop()
    adb("reverse", "--remove", f"tcp:{ctx.phone_port}")
    why = ctx.server.start()
    if why:
        return why
    ctx.phone.wake()
    adb("shell", "monkey", "-p", APP, "-c", "android.intent.category.LAUNCHER", "1")
    # A road dial that failed while the server restarted rests 60 s (the
    # bridge's INTERNET_REST) before the phone's next try.
    if not wait_for(lambda: iroh_loops(ctx.server.log()) and ctx.device in ctx.server.linked(), 200, 2):
        return "the phone did not link by the internet road within 200 s"
    return None


def start_session(ctx: Ctx) -> Optional[float]:
    """Tap Connect on the throwaway's card (127.0.0.1); the phone's time of
    the tap, in seconds of the day, or None when there is no card."""
    ctx.phone.wake()
    adb("shell", "monkey", "-p", APP, "-c", "android.intent.category.LAUNCHER", "1")
    # A session that dropped when its server stopped stays on its card
    # ("The connection to the computer dropped…"): left by its Exit first.
    if wait_for(lambda: any(t.startswith("The connection to the computer dropped")
                            for t, _, _ in dump_nodes(ctx.phone.dump())), 4, 2):
        tap_text("Exit", ctx.phone.dump())
        time.sleep(2)
    spot = wait_for(lambda: connect_button(ctx.phone.dump(), "127.0.0.1"), 20, 2)
    for _ in range(3):
        if spot:
            break
        # Another leftover screen (a kicked session's card, a sheet): Back
        # towards Home, and look again.
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        time.sleep(2)
        adb("shell", "monkey", "-p", APP, "-c", "android.intent.category.LAUNCHER", "1")
        spot = wait_for(lambda: connect_button(ctx.phone.dump(), "127.0.0.1"), 8, 2)
    if not spot:
        return None
    adb("logcat", "-c")
    now = adb("shell", "date", "+%H:%M:%S.%N").strip()
    hours, minutes, seconds = now.split(":")
    adb("shell", "input", "tap", str(spot[0]), str(spot[1]))
    return int(hours) * 3600 + int(minutes) * 60 + float(seconds[:6])


def end_session(ctx: Ctx) -> None:
    """Exit the session as its button does. Once its buttons are hidden, the
    server ends it (a kick, as `pocketdesk kick` asks) and Back leaves the
    card that says so."""
    exit_spot = next(((x, y) for text, x, y in dump_nodes(ctx.phone.dump()) if text == "Exit"), None)
    if exit_spot:
        adb("shell", "input", "tap", str(exit_spot[0]), str(exit_spot[1]))
    else:
        if in_session(ctx):
            folder = ctx.server.root / "state" / "kicks"
            folder.mkdir(exist_ok=True)
            name = str(uuid.uuid4())
            line = chr(10).join([ctx.device, str(int(time.time())), ""])
            (folder / f"{name}.tmp").write_text(line, encoding="utf-8")
            os.replace(folder / f"{name}.tmp", folder / f"{name}.kick")
            wait_for(lambda: not in_session(ctx), 15, 1)
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
    time.sleep(3)


def connected_at() -> Optional[float]:
    """When the phone's log says the session connected, from the last
    logcat clear."""
    for line in adb("logcat", "-d", "-s", "PocketdeskRust:*").splitlines():
        if "Streaming session connected" in line:
            return log_seconds(line)
    return None


def captions(ctx: Ctx, seconds: float) -> list[tuple[float, str]]:
    """The HUD's caption each time it changed, over `seconds`."""
    seen: list[tuple[float, str]] = []
    start = time.monotonic()
    while time.monotonic() - start < seconds:
        caption = hud_caption(ctx.phone.dump())
        if caption and (not seen or seen[-1][1] != caption):
            seen.append((round(time.monotonic() - start, 1), caption))
        time.sleep(0.5)
    return seen


def needs_details(ctx: Ctx) -> Optional[str]:
    """The HUD's numbers and caption show only with this on (off by default)."""
    if not ctx.phone.prefs("pocketdesk-ui").get("connection_details"):
        return "turn on Show connection details in the phone's Settings"
    return None


def in_session(ctx: Ctx) -> bool:
    """Whether the server has a session from the phone, by its fresh mirror
    (the phone hides the session's buttons once untouched, and a tap to
    show them would be a touch toward this computer)."""
    path = ctx.server.root / "state" / "live_sessions.json"
    try:
        mirror = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    if time.time() - mirror.get("updated_unix", 0) > 10:
        return False
    return any(s.get("device_id") == ctx.device for s in mirror.get("sessions", []))


def session_check(ctx: Ctx, body: Callable[[Ctx], tuple[str, str]]) -> tuple[str, str]:
    """Run `body` by the internet road with input off. The session checks
    are registered last and share the road: the phone stays on it until
    the runner stops the server and removes the reverse."""
    if not input_off_for(ctx):
        return "SKIP", "the phone's input is on in the throwaway's trust store"
    if not ctx.on_road:
        why = by_road(ctx)
        if why:
            return "TIMEOUT", why
        ctx.on_road = True
    try:
        return body(ctx)
    finally:
        end_session(ctx)


@check("w1-away-connect", "§11")
def w1_away_connect(ctx: Ctx) -> tuple[str, str]:
    """V8: a session from away connects within a few seconds of the tap."""
    def body(ctx: Ctx) -> tuple[str, str]:
        tapped = start_session(ctx)
        if tapped is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        up = wait_for(connected_at, 20, 0.5)
        if not up:
            return "TIMEOUT", "no session within 20 s of the tap"
        took = (up - tapped) % 86400
        return ("PASS" if took <= 6 else "FAIL"), f"connected {took:.1f} s after the tap"
    return session_check(ctx, body)


@check("w1-caption", "§2d")
def w1_caption(ctx: Ctx) -> tuple[str, str]:
    """The HUD's basis caption fills in: rates over a span, then an RTT
    median of a count."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        seen = captions(ctx, 25)
        if not seen:
            return "TIMEOUT", "no caption within 25 s"
        counts = [rtt_count(c) for _, c in seen]
        said = " → ".join(c for _, c in seen)
        if any(n and n > 0 for n in counts if n is not None):
            return "PASS", said
        return "FAIL", f"no RTT median within 25 s: {said}"
    why = needs_details(ctx)
    if why:
        return "SKIP", why
    return session_check(ctx, body)


@check("w1-reset", "§2d")
def w1_reset(ctx: Ctx) -> tuple[str, str]:
    """V7: a new session's caption starts from its own polls, never with the
    last session's count."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        first = captions(ctx, 20)
        before = max((rtt_count(c) or 0 for _, c in first), default=0)
        end_session(ctx)
        if start_session(ctx) is None:
            return "SKIP", "no Connect after the first session"
        early = captions(ctx, 6)
        if not early:
            return "TIMEOUT", "no caption in the new session's first 6 s"
        carried = [c for _, c in early if (rtt_count(c) or 0) >= max(before, 3)]
        said = " → ".join(c for _, c in early)
        return ("FAIL", f"carried over ({before} before): {said}") if carried else ("PASS", f"{before} before; then {said}")
    why = needs_details(ctx)
    if why:
        return "SKIP", why
    return session_check(ctx, body)


def pss_mb() -> Optional[float]:
    out = adb("shell", "dumpsys", "meminfo", APP)
    found = re.search(r"TOTAL PSS:\s+(\d+)", out) or re.search(r"^\s*TOTAL\s+(\d+)", out, re.M)
    return round(int(found.group(1)) / 1024, 1) if found else None


@check("w1-hour", "§16")
def w1_hour(ctx: Ctx) -> tuple[str, str]:
    """An hour's session (POCKETDESK_HOUR_MINUTES for less): the app's memory
    levels off, and the session is still up at the end."""
    minutes = int(os.environ.get("POCKETDESK_HOUR_MINUTES", "60"))

    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        time.sleep(20)
        samples = []
        start = time.monotonic()
        while True:
            samples.append(pss_mb())
            left = minutes * 60 - (time.monotonic() - start)
            if left <= 0:
                break
            time.sleep(min(300, left))
        up = in_session(ctx)
        mb = [s for s in samples if s is not None]
        if not mb:
            return "TIMEOUT", "no memory reading"
        grew = mb[-1] - mb[0]
        said = f"{minutes} min: {mb[0]} → {mb[-1]} MB (max {max(mb)})"
        if not up:
            return "FAIL", f"the session was gone at the end; {said}"
        return ("PASS" if grew <= 30 else "FAIL"), said
    why = needs_details(ctx)
    if why:
        return "SKIP", why
    return session_check(ctx, body)


LONG.add("w1-hour")


def bitrate_changes(log: str) -> int:
    """The server's lines that change a session's bitrate: the loss
    response's target and the adaptive rate (W2)."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    return sum(
        "Encoder bitrate target changed" in line or "Adaptive bitrate:" in line
        for line in plain.splitlines()
    )


@check("w2-quiet", "§11")
def w2_quiet(ctx: Ctx) -> tuple[str, str]:
    """No loss response on a quiet desktop: a session held a minute with
    nothing moving changes no bitrate (fewer than 100 datagrams a tick are
    no information). This computer's screen must be still meanwhile."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(connected_at, 20, 0.5):
            return "TIMEOUT", "no session within 20 s of the tap"
        time.sleep(15)
        before = bitrate_changes(ctx.server.log())
        time.sleep(60)
        if not in_session(ctx):
            return "FAIL", "the session was gone after the minute"
        new = bitrate_changes(ctx.server.log()) - before
        if new == 0:
            return "PASS", "no bitrate change in a quiet minute"
        # Its words: a relayed road going direct lifts its cap with the same
        # line (`relayed=`), which is not a loss response (W2 review).
        plain = re.sub(chr(27) + r"\[[0-9;]*m", "", ctx.server.log()).splitlines()
        last = next((l for l in reversed(plain) if "bitrate target changed" in l or "Adaptive bitrate:" in l), "")
        return "FAIL", f"{new} bitrate change(s) in a quiet minute; the last: {last[-160:]}"
    return session_check(ctx, body)


# ---- long transfers over the internet road (W3) ----

PHONE_DOWNLOADS = "/sdcard/Download/Pocketdesk"
GIB = 1 << 30


def big_file(ctx: Ctx, size: int) -> pathlib.Path:
    """A file of random bytes in the throwaway's folder, named for this run."""
    path = ctx.server.root / f"w3-{secrets.token_hex(3)}.bin"
    try:
        with open(path, "wb") as out:
            left = size
            while left:
                chunk = min(left, 8 << 20)
                out.write(os.urandom(chunk))
                left -= chunk
    except BaseException:
        # A full disk or an interrupt leaves no half gigabyte behind (W3 review).
        path.unlink(missing_ok=True)
        raise
    return path


def md5_of(path: pathlib.Path) -> str:
    import hashlib
    digest = hashlib.md5()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(8 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def send_cli(ctx: Ctx, path: pathlib.Path, wait: Optional[int]) -> subprocess.Popen:
    """`pocketdesk send` to the phone, as the Files tab sends."""
    args = [str(ctx.server.exe), "send", ctx.device, str(path)] + (["--wait", str(wait)] if wait else [])
    full = dict(os.environ)
    full.update(ctx.server.env())
    return subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=full, text=True)


def transfer_done(ctx: Ctx, name: str) -> Optional[tuple[int, int]]:
    """`(done, size)` of the transfer of `name`, from the server's record."""
    try:
        record = json.loads((ctx.server.root / "state" / "transfers.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    found = next((t for t in record.get("transfers", []) if t.get("name") == name), None)
    return (found.get("done", 0), found.get("size", 0)) if found else None


def on_phone(name: str) -> list[str]:
    """The entries in the phone's Pocketdesk downloads that carry `name`."""
    out = adb("shell", f"ls -a {PHONE_DOWNLOADS} 2>/dev/null")
    return [entry for entry in out.split() if name in entry]


def drop_from_phone(name: str) -> None:
    for entry in on_phone(name):
        adb("shell", device_command("rm", "-f", f"{PHONE_DOWNLOADS}/{entry}"))


@check("w3-big-send", "§31")
def w3_big_send(ctx: Ctx) -> tuple[str, str]:
    """A file over 1 GB to the phone in a session by the internet road: all
    of it, the same bytes (size and MD5 on the phone)."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        path = big_file(ctx, GIB + (1 << 20))
        try:
            want = md5_of(path)
            started = time.monotonic()
            sent = send_cli(ctx, path, 1200)
            try:
                out, _ = sent.communicate(timeout=1260)
            except subprocess.TimeoutExpired:
                sent.kill()
                return "TIMEOUT", "the send ran past 21 minutes"
            took = time.monotonic() - started
            if sent.returncode != 0:
                return "FAIL", f"send exit {sent.returncode}: {out.strip()[-200:]}"
            remote = f"{PHONE_DOWNLOADS}/{path.name}"
            size = adb("shell", device_command("stat", "-c", "%s", remote)).strip()
            got = adb("shell", device_command("md5sum", remote), timeout=300).split()
            if got[:1] == ["TIMEOUT"]:
                return "TIMEOUT", "md5sum on the phone ran past 300 s"
            same = size == str(path.stat().st_size) and got and got[0] == want
            rate = path.stat().st_size / took / (1 << 20)
            return ("PASS" if same else "FAIL"), f"{size} bytes in {took:.0f} s ({rate:.1f} MiB/s), MD5 {'matches' if same else 'differs'}"
        finally:
            drop_from_phone(path.name)
            path.unlink(missing_ok=True)
    return session_check(ctx, body)


@check("w3-cut-send", "§31")
def w3_cut_send(ctx: Ctx) -> tuple[str, str]:
    """A send to the phone cut part way (the server stopped, as a dropped
    network would): no file published under its name."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        path = big_file(ctx, GIB)
        try:
            sent = send_cli(ctx, path, None)
            try:
                sent.communicate(timeout=60)
            except subprocess.TimeoutExpired:
                sent.kill()
            part = wait_for(lambda: (d := transfer_done(ctx, path.name)) and d[1] and d[0] >= d[1] * 0.3 and d, 600, 1)
            if not part:
                return "TIMEOUT", "the send never reached 30 %"
            # The cut: the server goes, mid-send; the road is set up again
            # by the next session check.
            ctx.server.stop()
            ctx.on_road = False
            time.sleep(30)
            left = on_phone(path.name)
            published = [e for e in left if e == path.name]
            said = f"cut at {part[0] * 100 // part[1]} %; on the phone: {len(left)} entr{'y' if len(left) == 1 else 'ies'} with its name"
            return ("FAIL", f"published: {said}") if published else ("PASS", said)
        finally:
            drop_from_phone(path.name)
            path.unlink(missing_ok=True)
            if ctx.server.child is None or ctx.server.child.poll() is not None:
                ctx.server.start()
    return session_check(ctx, body)


LONG.update({"w3-big-send", "w3-cut-send"})


# ---- sharing from the phone (X1) ----
#
# The fixture app shares one of its own files to Pocketdesk's ShareActivity,
# naming the throwaway (127.0.0.1): never "the last computer used", which
# could be the owner's. (The adb shell cannot grant Pocketdesk a MediaStore
# file: Android refuses, and Pocketdesk says the file is unreadable.) Naming
# one needs the phone's "Let other apps choose a computer". At home the
# computer would dial the phone back for the file, which adb reverse cannot
# carry, so these too go by the internet road.

FIXTURE_STAGE = "/data/local/tmp"
NL = chr(10)


def stage_in_fixture(local: pathlib.Path) -> None:
    """Put `local` in the fixture's files folder, which it shares from."""
    adb("push", str(local), f"{FIXTURE_STAGE}/{local.name}", timeout=900)
    adb("shell", device_command("run-as", FIXTURE, "cp", f"{FIXTURE_STAGE}/{local.name}", f"files/{local.name}"), timeout=600)
    adb("shell", device_command("rm", "-f", f"{FIXTURE_STAGE}/{local.name}"))


def unstage(name: str) -> None:
    adb("shell", device_command("run-as", FIXTURE, "rm", "-f", f"files/{name}"))


def share_to_throwaway(name: str) -> None:
    """The fixture shares its file to Pocketdesk, naming the throwaway."""
    adb("shell", device_command("am", "start", "-n", f"{FIXTURE}/.ShareOutActivity",
                                "--es", "name", name, "--es", "address", "127.0.0.1"))


def needs_choosing(ctx: Ctx) -> Optional[str]:
    if not ctx.phone.prefs("pocketdesk-ui").get("others_choose_computer"):
        return "turn on Let other apps choose a computer in the phone's Settings"
    return None


def road_check(ctx: Ctx, body: Callable[[Ctx], tuple[str, str]]) -> tuple[str, str]:
    """Run `body` with the phone linked by the internet road (no session)."""
    if not ctx.on_road:
        why = by_road(ctx)
        if why:
            return "TIMEOUT", why
        ctx.on_road = True
    return body(ctx)


def shared(ctx: Ctx, local: pathlib.Path, deadline: float) -> tuple[Optional[pathlib.Path], str]:
    """Push `local` to the phone, share it to the throwaway, and wait for it
    in the throwaway's downloads at its full size. The phone's copy is
    removed after."""
    name = local.name
    stage_in_fixture(local)
    try:
        adb("logcat", "-c")
        share_to_throwaway(name)
        landed = ctx.server.root / "downloads" / name
        size = local.stat().st_size
        if not wait_for(lambda: landed.exists() and landed.stat().st_size == size, deadline, 2):
            # AA2: what the phone did with it, in its own words.
            said = [re.sub(r"^\S+ \S+\s+\d+\s+\d+ ", "", line).strip()
                    for line in adb("logcat", "-d", "-s", "ShareActivity:*", "TransferService:*").splitlines()
                    if "ShareActivity" in line or "TransferService" in line]
            # A stack trace's frames are lines too: they would crowd out the
            # job's own words (AA2 review).
            said = [line for line in said if not line.split(": ", 1)[-1].lstrip().startswith("at ")]
            phone = "; ".join(said[-4:]) or "nothing from ShareActivity or TransferService"
            return None, f"not in the throwaway's downloads at full size within {deadline:.0f} s; the phone: {phone}"
        return landed, ""
    finally:
        unstage(name)


@check("x1-share-big", "§31")
def x1_share_big(ctx: Ctx) -> tuple[str, str]:
    """A file over 1 GB from the phone to the computer by the internet road:
    all of it, the same bytes."""
    why = needs_choosing(ctx)
    if why:
        return "SKIP", why

    def body(ctx: Ctx) -> tuple[str, str]:
        local = big_file(ctx, GIB + (1 << 20))
        local = local.rename(local.with_name("x1-" + local.name[3:]))
        landed = None
        try:
            want = md5_of(local)
            started = time.monotonic()
            landed, why = shared(ctx, local, 1200)
            if not landed:
                return "TIMEOUT", why
            took = time.monotonic() - started
            same = md5_of(landed) == want
            return ("PASS" if same else "FAIL"), f"{landed.stat().st_size} bytes in {took:.0f} s (with the push), MD5 {'matches' if same else 'differs'}"
        finally:
            local.unlink(missing_ok=True)
            if landed:
                landed.unlink(missing_ok=True)
    return road_check(ctx, body)


@check("x1-file-received", "§78")
def x1_file_received(ctx: Ctx) -> tuple[str, str]:
    """`file_received` at both detail levels: the size always, the file's
    name only with events_detail = "full"."""
    why = needs_choosing(ctx)
    if why:
        return "SKIP", why
    before = None

    def body(ctx: Ctx) -> tuple[str, str]:
        nonlocal before
        said = []
        try:
            for detail in ("metadata", "full"):
                # The runner's helper: keys at the top level, written whole (X1 review).
                was = ctx.server.configure({"events": "true", "events_detail": f'"{detail}"'})
                before = was if before is None else before
                time.sleep(2)
                local = ctx.server.root / f"x1-{detail}-{secrets.token_hex(3)}.txt"
                local.write_text("x1" + NL, encoding="utf-8")
                count = ctx.server.event_count()
                landed = None
                try:
                    landed, why = shared(ctx, local, 120)
                    if not landed:
                        return "TIMEOUT", f"{detail}: {why}"
                    event = wait_for(lambda: next((e for e in ctx.server.events_after_count(count)
                                                   if e.get("event") == "file_received"), None), 20, 1)
                    if not event:
                        return "TIMEOUT", f"{detail}: no file_received event"
                    has_name = "name" in event
                    if has_name != (detail == "full") or "size" not in event:
                        return "FAIL", f"{detail}: the event's fields were {sorted(event)}"
                    said.append(f"{detail}: size{' and name' if has_name else ', no name'}")
                finally:
                    local.unlink(missing_ok=True)
                    if landed:
                        landed.unlink(missing_ok=True)
            return "PASS", "; ".join(said)
        finally:
            if before is not None:
                ctx.server.restore_config(before)
    return road_check(ctx, body)


LONG.add("x1-share-big")


# ---- a cut transfer continued (X2) ----


@check("x2-resume", "§50")
def x2_resume(ctx: Ctx) -> tuple[str, str]:
    """A send to the phone cut part way (the server stopped, as a dropped
    network would) and sent again: the phone continues from what it kept,
    and the file ends whole (§46, §50)."""
    if not ctx.phone.prefs("pocketdesk-ui").get("retry_transfers", True):
        return "SKIP", "turn on Retry failed files in the phone's Settings"

    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        path = big_file(ctx, GIB)
        settled = False
        try:
            want = md5_of(path)
            first = send_cli(ctx, path, None)
            try:
                first.communicate(timeout=60)
            except subprocess.TimeoutExpired:
                first.kill()
            part = wait_for(lambda: (d := transfer_done(ctx, path.name)) and d[1] and d[0] >= d[1] * 0.3 and d, 600, 1)
            if not part:
                return "TIMEOUT", "the first send never reached 30 %"
            # The cut, as in w3-cut-send, but the partial stays on the phone.
            ctx.server.stop()
            # Off the road until it is set up again (X2 review: a failed
            # relink must not leave the next check thinking it is on it).
            ctx.on_road = False
            time.sleep(15)
            why = by_road(ctx)
            if why:
                return "TIMEOUT", f"after the cut: {why}"
            ctx.on_road = True
            # The phone is still on the dropped session's card: leave it.
            end_session(ctx)
            if start_session(ctx) is None:
                return "TIMEOUT", "no Connect after the cut"
            if not wait_for(lambda: in_session(ctx), 30, 1):
                return "TIMEOUT", "no session after the cut"
            # start_session cleared the phone's log: what follows is this send's.
            second = send_cli(ctx, path, 1200)
            try:
                out, _ = second.communicate(timeout=1260)
            except subprocess.TimeoutExpired:
                second.kill()
                return "TIMEOUT", "the second send ran past 21 minutes"
            settled = True
            if second.returncode != 0:
                return "FAIL", f"the second send's exit {second.returncode}: {out.strip()[-200:]}"
            said = adb("logcat", "-d", "-s", "PocketdeskRust:*")
            found = re.search(r"Download continued from (\d+) of (\d+) bytes", said)
            remote = f"{PHONE_DOWNLOADS}/{path.name}"
            got = adb("shell", device_command("md5sum", remote), timeout=300).split()
            if got[:1] == ["TIMEOUT"]:
                return "TIMEOUT", "md5sum on the phone ran past 300 s"
            same = bool(got) and got[0] == want
            cut = f"cut at {part[0] * 100 // part[1]} %"
            if not found:
                return "FAIL", f"{cut}; sent again whole (no \"continued\" in the phone's log), MD5 {'matches' if same else 'differs'}"
            offset = int(found.group(1))
            return ("PASS" if same and offset > 0 else "FAIL"), (
                f"{cut}; continued from {offset * 100 // int(found.group(2))} %, MD5 {'matches' if same else 'differs'}")
        finally:
            if not settled:
                # A send may still run: stopped first (AC2 review).
                ctx.server.stop()
                ctx.on_road = False
            drop_from_phone(path.name)
            path.unlink(missing_ok=True)
    return session_check(ctx, body)


LONG.add("x2-resume")


# ---- a share from the phone cut and continued (AC1) ----


def continued_from(log: str, name: str) -> Optional[tuple[int, int]]:
    """The offset and size the server's log says it continued `name` from
    (its colour codes stripped), or None when it did not."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    found = re.search(r"Continued " + re.escape(name) + r" from (\d+) of (\d+) bytes", plain)
    return (int(found.group(1)), int(found.group(2))) if found else None


def partial_bytes(folder: pathlib.Path, before: frozenset = frozenset()) -> int:
    """The largest unfinished file in the server's `partials` folder, of
    those not named in `before` (AC1 review: a leftover must not pass for
    this share's)."""
    if not folder.is_dir():
        return 0
    return max((p.stat().st_size for p in folder.glob("*.part") if p.name not in before), default=0)


def part_names(folder: pathlib.Path) -> frozenset:
    return frozenset(p.name for p in folder.glob("*.part")) if folder.is_dir() else frozenset()


def forget_partials(folder: pathlib.Path, name: str) -> None:
    """Remove what the server kept of `name`, its record and its bytes."""
    if not folder.is_dir():
        return
    for record in folder.glob("*.json"):
        try:
            kept = json.loads(record.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if kept.get("name") == name:
            if kept.get("location"):
                (folder / kept["location"]).unlink(missing_ok=True)
            record.unlink(missing_ok=True)


def cut_share(ctx: Ctx, kept: bool) -> tuple[str, str]:
    """Share 1 GiB from the phone by the internet road, stop the throwaway at
    30 % (as a dropped network would), start it again, and let the phone's
    retry finish it; shared again from the same file when the phone's
    retries ran out while the server was away. With unfinished files `kept`
    the computer continues from what it had, else it starts over."""
    partials = ctx.server.root / "state" / "partials"
    local = big_file(ctx, GIB)
    local = local.rename(local.with_name("ac1-" + local.name[3:]))
    landed = ctx.server.root / "downloads" / local.name
    size = local.stat().st_size
    before = part_names(partials)
    settled = False
    try:
        want = md5_of(local)
        stage_in_fixture(local)
        adb("logcat", "-c")
        share_to_throwaway(local.name)
        if not wait_for(lambda: partial_bytes(partials, before) >= size * 0.3, 600, 1):
            return "TIMEOUT", "the share never reached 30 % on the computer"
        cut = partial_bytes(partials, before) * 100 // size
        ctx.server.stop()
        # Off the road until it is set up again (as in x2-resume).
        ctx.on_road = False
        if landed.exists():
            # AC1 review: a share that ended before the stop was never cut.
            return "TIMEOUT", "the share finished before the cut"
        why = by_road(ctx)
        if why:
            return "TIMEOUT", f"after the cut: {why}"
        ctx.on_road = True
        how = "the phone's retry"
        # Wait while the file grows; share again only once it has stood
        # still for two minutes (the phone's retries, 5, 15 and 45 s apart,
        # can run out while the road rests), never over a retry still
        # sending (AC1 review). The same file has the same source id.
        deadline = time.monotonic() + 1500
        last, since = -1, time.monotonic()
        while not (landed.exists() and landed.stat().st_size == size):
            if time.monotonic() > deadline:
                return "TIMEOUT", f"cut at {cut} %; not whole in the throwaway's downloads within 25 minutes ({how})"
            now = partial_bytes(partials, before)
            if now != last:
                last, since = now, time.monotonic()
            elif time.monotonic() - since > 120 and how != "shared again":
                how = "shared again"
                share_to_throwaway(local.name)
                since = time.monotonic()
            time.sleep(2)
        settled = True
        same = md5_of(landed) == want
        found = continued_from(ctx.server.log(), local.name)
        said = f"cut at {cut} %; {how}"
        if kept:
            if not found:
                return "FAIL", f"{said} started over (no \"Continued\" in the server's log), MD5 {'matches' if same else 'differs'}"
            return ("PASS" if same and found[0] > 0 else "FAIL"), (
                f"{said} continued from {found[0] * 100 // found[1]} %, MD5 {'matches' if same else 'differs'}")
        if found:
            return "FAIL", f"{said} continued from {found[0] * 100 // found[1]} % with unfinished files not kept"
        return ("PASS" if same else "FAIL"), f"{said} started over, MD5 {'matches' if same else 'differs'}"
    finally:
        if not settled:
            # A transfer may still run: stopped first, so nothing lands
            # after the cleanup (AC1 review).
            ctx.server.stop()
            ctx.on_road = False
        unstage(local.name)
        local.unlink(missing_ok=True)
        landed.unlink(missing_ok=True)
        forget_partials(partials, local.name)
        # Parts with no record (unfinished files not kept) go too.
        for name in part_names(partials) - before:
            (partials / name).unlink(missing_ok=True)


def share_resume_ready(ctx: Ctx) -> Optional[str]:
    why = needs_choosing(ctx)
    if why:
        return why
    if not ctx.phone.prefs("pocketdesk-ui").get("retry_transfers", True):
        return "turn on Retry failed files in the phone's Settings"
    return None


@check("ac1-share-resume", "§50")
def ac1_share_resume(ctx: Ctx) -> tuple[str, str]:
    """A share from the phone cut part way and sent again: the computer
    continues from what it kept, and the file ends whole (§46, §50)."""
    why = share_resume_ready(ctx)
    if why:
        return "SKIP", why
    return road_check(ctx, lambda ctx: cut_share(ctx, kept=True))


@check("ac1-not-kept", "§50")
def ac1_not_kept(ctx: Ctx) -> tuple[str, str]:
    """With `transfer_keep_hours = 0` (unfinished files not kept), a share
    cut part way starts over when sent again."""
    why = share_resume_ready(ctx)
    if why:
        return "SKIP", why
    # Read at the server's start: the cut's restart takes it.
    before = ctx.server.configure({"transfer_keep_hours": "0"})
    try:
        return road_check(ctx, lambda ctx: cut_share(ctx, kept=False))
    finally:
        ctx.server.restore_config(before)
        # The running server still keeps nothing: the next road check starts
        # it again with the config put back.
        ctx.on_road = False


LONG.update({"ac1-share-resume", "ac1-not-kept"})


# ---- a file changed between attempts (AC2) ----


def change_start(path: pathlib.Path) -> None:
    """New bytes in the file's first MiB, its size and modified time kept,
    so its source id stays the same and only the prefix hash can tell."""
    seen = path.stat()
    with open(path, "r+b") as f:
        f.write(os.urandom(1 << 20))
    os.utime(path, ns=(seen.st_atime_ns, seen.st_mtime_ns))


@check("ac2-changed", "§50")
def ac2_changed(ctx: Ctx) -> tuple[str, str]:
    """A send to the phone cut part way, the file's start changed (its size
    and date kept), and sent again: the phone sees it changed (the prefix
    hash), starts over, and ends with the new file (§50)."""
    if not ctx.phone.prefs("pocketdesk-ui").get("retry_transfers", True):
        return "SKIP", "turn on Retry failed files in the phone's Settings"

    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        path = big_file(ctx, GIB)
        settled = False
        try:
            first = send_cli(ctx, path, None)
            try:
                first.communicate(timeout=60)
            except subprocess.TimeoutExpired:
                first.kill()
            part = wait_for(lambda: (d := transfer_done(ctx, path.name)) and d[1] and d[0] >= d[1] * 0.3 and d, 600, 1)
            if not part:
                return "TIMEOUT", "the first send never reached 30 %"
            ctx.server.stop()
            ctx.on_road = False
            change_start(path)
            want = md5_of(path)
            time.sleep(15)
            why = by_road(ctx)
            if why:
                return "TIMEOUT", f"after the cut: {why}"
            ctx.on_road = True
            end_session(ctx)
            if start_session(ctx) is None:
                return "TIMEOUT", "no Connect after the cut"
            if not wait_for(lambda: in_session(ctx), 30, 1):
                return "TIMEOUT", "no session after the cut"
            second = send_cli(ctx, path, 1200)
            try:
                out, _ = second.communicate(timeout=1260)
            except subprocess.TimeoutExpired:
                second.kill()
                return "TIMEOUT", "the second send ran past 21 minutes"
            settled = True
            said = adb("logcat", "-d", "-s", "PocketdeskRust:*")
            # Its words reach the phone's log through the job's failure, and
            # the server's through the send's (either is the prefix hash).
            noticed = any("changed since the last attempt" in log
                          for log in (adb("logcat", "-d"), ctx.server.log()))
            continued = re.search(r"Download continued from (\d+) of (\d+) bytes", said)
            remote = f"{PHONE_DOWNLOADS}/{path.name}"
            got = adb("shell", device_command("md5sum", remote), timeout=300).split()
            if got[:1] == ["TIMEOUT"]:
                return "TIMEOUT", "md5sum on the phone ran past 300 s"
            same = bool(got) and got[0] == want
            cut = f"cut at {part[0] * 100 // part[1]} %"
            told = "the change was seen" if noticed else "no word of the change in either log"
            if continued:
                return "FAIL", f"{cut}; {told}, but continued from {int(continued.group(1)) * 100 // int(continued.group(2))} %, MD5 of the new file {'matches' if same else 'differs'}"
            if second.returncode != 0:
                return "FAIL", f"{cut}; {told}; the second send's exit {second.returncode}: {out.strip()[-200:]}"
            return ("PASS" if same and noticed else "FAIL"), f"{cut}; {told} and started over, MD5 of the new file {'matches' if same else 'differs'}"
        finally:
            if not settled:
                # A send may still run: stopped first, so the cleanup does
                # not race it (AC2 review, as in cut_share).
                ctx.server.stop()
                ctx.on_road = False
            drop_from_phone(path.name)
            path.unlink(missing_ok=True)
    return session_check(ctx, body)


LONG.add("ac2-changed")


# ---- a send to the phone during a share from it (AC3) ----


@check("ac3-send-during-share", "§31")
def ac3_send_during_share(ctx: Ctx) -> tuple[str, str]:
    """A send to the phone while a large share from the phone runs, by the
    internet road: the offer still arrives, the file the same bytes on the
    phone, and the share ends whole too (§31). The phone moves one file at
    a time (TransferService's one worker), so the send waits behind the
    share; the computer's offer lasts 10 minutes (OFFER_TTL), which a
    gigabyte by the road takes about two of. Outside a session the phone
    takes offers only with its Files from the computer in the background
    on (its first run, without it, waited out the send)."""
    why = needs_choosing(ctx)
    if why:
        return "SKIP", why
    if not ctx.phone.prefs("pocketdesk-ui").get("files_in_background"):
        return "SKIP", "turn on Files from the computer in the background in the phone's Settings"

    def body(ctx: Ctx) -> tuple[str, str]:
        partials = ctx.server.root / "state" / "partials"
        big = big_file(ctx, GIB)
        big = big.rename(big.with_name("ac3-" + big.name[3:]))
        small = ctx.server.root / f"ac3-small-{secrets.token_hex(3)}.bin"
        landed = ctx.server.root / "downloads" / big.name
        size = big.stat().st_size
        before = part_names(partials)
        settled = False
        try:
            with open(small, "wb") as out:
                out.write(os.urandom(1 << 20))
            want_big, want_small = md5_of(big), md5_of(small)
            stage_in_fixture(big)
            share_to_throwaway(big.name)
            if not wait_for(lambda: partial_bytes(partials, before) >= size * 0.1, 600, 1):
                return "TIMEOUT", "the share never reached 10 % on the computer"
            at = partial_bytes(partials, before) * 100 // size
            if landed.exists():
                return "SKIP", "the share ended before the send began: nothing overlapped"
            started = time.monotonic()
            sent = send_cli(ctx, small, 900)
            try:
                out, _ = sent.communicate(timeout=930)
            except subprocess.TimeoutExpired:
                sent.kill()
                return "TIMEOUT", f"the send during the share (from {at} %) ran past 930 s"
            took = time.monotonic() - started
            # Which came first: the queued send, or the share it waited for.
            order = "after the share" if landed.exists() else "before the share ended"
            # The phone publishes the file just after the send hears it saved
            # (AC3 review): a few seconds for it to appear.
            # Until it hashes as sent (review: a half-published copy must not
            # end the wait).
            small_same = bool(wait_for(lambda: want_small in adb(
                "shell", device_command("md5sum", f"{PHONE_DOWNLOADS}/{small.name}"), timeout=120).split(), 20, 2))
            if not wait_for(lambda: landed.exists() and landed.stat().st_size == size, 1200, 2):
                return "TIMEOUT", "the share did not end whole within 20 minutes of the send"
            settled = True
            big_same = md5_of(landed) == want_big
            said = (f"sent during the share at {at} %: done in {took:.0f} s, {order}, exit {sent.returncode},"
                    f" MD5 {'matches' if small_same else 'differs'}; the share whole, MD5 {'matches' if big_same else 'differs'}")
            if sent.returncode != 0:
                return "FAIL", said + f"; the send said: {out.strip()[-200:]}"
            return ("PASS" if small_same and big_same else "FAIL"), said
        finally:
            if not settled:
                # The share may still run: stopped first (AC1 review).
                ctx.server.stop()
                ctx.on_road = False
            unstage(big.name)
            drop_from_phone(small.name)
            for path in (big, small, landed):
                path.unlink(missing_ok=True)
            forget_partials(partials, big.name)
            for name in part_names(partials) - before:
                (partials / name).unlink(missing_ok=True)
    return road_check(ctx, body)


LONG.add("ac3-send-during-share")


# ---- files' dates from the phone (AC4) ----

# A date in the past for the staged file, as `touch -t` takes it.
PAST = "202001021230.00"


def dated_as(landed: float, original: float, arrived: float) -> str:
    """How a received file is dated: its own date ("kept", within 2 s), the
    time it arrived ("arrival", the 10 minutes before `arrived`), or
    "other"."""
    if abs(landed - original) <= 2:
        return "kept"
    if arrived - 600 <= landed <= arrived + 2:
        return "arrival"
    return "other"


def dated_share(ctx: Ctx) -> tuple[Optional[str], str]:
    """Share a small file dated in the past from the phone; how the
    computer dated it, or None and why it did not arrive."""
    local = ctx.server.root / f"ac4-{secrets.token_hex(3)}.txt"
    landed = ctx.server.root / "downloads" / local.name
    settled = False
    try:
        local.write_text("ac4" + NL, encoding="utf-8")
        stage_in_fixture(local)
        staged = f"files/{local.name}"
        adb("shell", device_command("run-as", FIXTURE, "touch", "-t", PAST, staged))
        stamp = adb("shell", device_command("run-as", FIXTURE, "stat", "-c", "%Y", staged)).strip()
        if not stamp.isdigit():
            return None, f"the staged file's date could not be read: {stamp[-80:]}"
        share_to_throwaway(local.name)
        if not wait_for(lambda: landed.exists() and landed.stat().st_size == local.stat().st_size, 120, 2):
            return None, "not in the throwaway's downloads within 120 s"
        settled = True
        # The date is set once the file is in place: a moment to settle.
        time.sleep(2)
        return dated_as(landed.stat().st_mtime, int(stamp), time.time()), ""
    finally:
        if not settled:
            # A late share must not land after the cleanup (AC3/AC4 review).
            ctx.server.stop()
            ctx.on_road = False
        unstage(local.name)
        local.unlink(missing_ok=True)
        landed.unlink(missing_ok=True)


@check("ac4-dates", "§149")
def ac4_dates(ctx: Ctx) -> tuple[str, str]:
    """A share from the phone dated in the past keeps its date while both
    sides keep dates, and is dated as it arrives with the phone's Keep
    files' dates off, or the computer's `keep_file_dates` off (K1)."""
    why = needs_choosing(ctx)
    if why:
        return "SKIP", why
    phone_keeps = bool(ctx.phone.prefs("pocketdesk-ui").get("keep_file_dates", True))

    def body(ctx: Ctx) -> tuple[str, str]:
        said = []
        # Read at each file: no restart needed.
        for computer_keeps in (True, False):
            before = ctx.server.configure({"keep_file_dates": "true" if computer_keeps else "false"})
            try:
                got, why = dated_share(ctx)
            finally:
                ctx.server.restore_config(before)
            if got is None:
                return "TIMEOUT", f"the computer's dates {'on' if computer_keeps else 'off'}: {why}"
            want = "kept" if phone_keeps and computer_keeps else "arrival"
            said.append(f"the computer's dates {'on' if computer_keeps else 'off'}: {got}")
            if got != want:
                return "FAIL", f"the phone's dates {'on' if phone_keeps else 'off'}; " + "; ".join(said) + f" (wanted {want})"
        return "PASS", f"the phone's dates {'on' if phone_keeps else 'off'}; " + "; ".join(said)
    return road_check(ctx, body)


# ---- a session's codec (X3) ----


def negotiated(log: str) -> Optional[tuple[int, str]]:
    """The last `(caps, codec)` the server's log says a session negotiated."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    found = None
    for line in plain.splitlines():
        if "Client codec caps negotiated" in line:
            caps = re.search(r"caps=(\d+)", line)
            # The log quotes a string field: "H.265 (HEVC)" has a space.
            codec = re.search(r'codec="([^"]*)"', line) or re.search(r"codec=(\S+)", line)
            if caps and codec:
                found = (int(caps.group(1)), codec.group(1))
    return found


@check("x3-codec", "§2")
def x3_codec(ctx: Ctx) -> tuple[str, str]:
    """H.264 by default, even with a phone that can decode HEVC; with
    `allow_hevc` on, HEVC for such a phone (R4 C1)."""
    def session_codec(ctx: Ctx) -> tuple[Optional[tuple[int, str]], Optional[bool]]:
        """The session's `(caps, codec)` and whether it is up 8 s on; `up`
        None when there was no card to connect."""
        # Only this session's line: an earlier session check left its own in
        # the log (X3 review).
        before = ctx.server.log().count("Client codec caps negotiated")
        if start_session(ctx) is None:
            return None, None
        said = wait_for(lambda: ctx.server.log().count("Client codec caps negotiated") > before
                        and negotiated(ctx.server.log()), 30, 1)
        time.sleep(8)
        return said, in_session(ctx)

    def body(ctx: Ctx) -> tuple[str, str]:
        first, up = session_codec(ctx)
        if up is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not first:
            return "TIMEOUT", "no codec negotiated in the first session"
        caps, codec = first
        if codec != "H.264":
            return "FAIL", f"by default the session was {codec} (caps {caps})"
        end_session(ctx)
        before = ctx.server.configure({"allow_hevc": "true"})
        try:
            # Read at the server's start: a restart, and the road again.
            why = by_road(ctx)
            if why:
                return "TIMEOUT", f"with HEVC allowed: {why}"
            end_session(ctx)
            second, up = session_codec(ctx)
            if not second:
                return "TIMEOUT", "no codec negotiated with HEVC allowed"
            caps2, codec2 = second
            hevc_phone = bool(caps2 & 1)
            want = "H.265 (HEVC)" if hevc_phone else "H.264"
            said = f"default {codec}; allowed {codec2} (the phone {'can' if hevc_phone else 'cannot'} decode HEVC), the session {'up' if up else 'gone'} after 8 s"
            if codec2 != want:
                return "FAIL", said
            return ("PASS" if up else "FAIL"), said
        finally:
            ctx.server.restore_config(before)
            # The next session check starts the server again, without HEVC.
            ctx.on_road = False
    return session_check(ctx, body)


# ---- the phone's folders at scale (Y1) ----


def shared_folder(ctx: Ctx, writable: bool) -> Optional[tuple[int, str]]:
    """`(root id, path on the phone)` of a folder the phone shares (its
    Folders the computer may open), when its tree is on the phone's own
    storage."""
    import urllib.parse
    raw = ctx.phone.prefs("pocketdesk-phone-folders").get("folders")
    try:
        folders = json.loads(raw) if isinstance(raw, str) else []
    except ValueError:
        return None
    for folder in folders:
        if bool(folder.get("add")) != writable:
            continue
        tree = urllib.parse.unquote(str(folder.get("uri", "")).rsplit("/tree/", 1)[-1])
        if tree.startswith("primary:"):
            return int(folder.get("id", 0)), "/sdcard/" + tree[len("primary:"):]
    return None


def ask_phone_files(ctx: Ctx, kind: str, root: int, path: list[str]) -> None:
    """As the Phone files page asks (core::local::phone_files::request)."""
    folder = ctx.server.root / "state" / "phone_files_out"
    folder.mkdir(exist_ok=True)
    rid = str(uuid.uuid4())
    body = {"id": rid, "device_id": ctx.device, "kind": kind, "root": root, "path": path,
            "created_unix": int(time.time())}
    tmp = folder / f"{rid}.json.tmp"
    tmp.write_text(json.dumps(body), encoding="utf-8")
    os.replace(tmp, folder / f"{rid}.json")


def kept_listing(ctx: Ctx, path: list[str], after: float) -> Optional[dict]:
    """The phone's listing of `path` the server kept since `after`."""
    try:
        phones = json.loads((ctx.server.root / "state" / "phone_files.json").read_text(encoding="utf-8")).get("phones", {})
    except (OSError, ValueError):
        return None
    kept = phones.get(ctx.device)
    if not kept or kept.get("at_unix", 0) < int(after):
        return None
    listing = kept.get("listing", {})
    return listing if listing.get("path") == path else None


def y1_place(found: tuple[int, str]) -> tuple[int, str, str]:
    """Where a new folder goes in the shared folder `found`: `(root, its
    path on the phone, its name)`."""
    root, base = found
    name = "y1-" + secrets.token_hex(3)
    return root, f"{base}/{name}", name


def y1_fill(where: str, count: int) -> None:
    """Make the folder `where` on the phone with `count` small files (inside
    the caller's cleanup, Y1 review)."""
    adb("shell", f"mkdir -p {shlex.quote(where)} && cd {shlex.quote(where)} && "
                 f"i=1; while [ $i -le {count} ]; do echo y1 > f$i.txt; i=$((i+1)); done", timeout=600)


@check("y1-big-folder", "§70")
def y1_big_folder(ctx: Ctx) -> tuple[str, str]:
    """A shared folder of more than 2000 entries: the listing carries the
    first 2000 (MAX_ENTRIES) and says there are more (`truncated`), which
    the Phone files page shows."""
    found = shared_folder(ctx, writable=False)
    if not found:
        return "SKIP", "no read-only folder on the phone's storage in its Folders the computer may open"
    root, where, name = y1_place(found)
    try:
        y1_fill(where, 2100)
        asked = time.time()
        ask_phone_files(ctx, "list", root, [name])
        listing = wait_for(lambda: kept_listing(ctx, [name], asked), 60, 1)
        if not listing:
            return "TIMEOUT", "no listing of the folder within 60 s"
        if listing.get("error"):
            return "FAIL", f"the listing said: {listing['error']}"
        got = len(listing.get("entries", []))
        cut = bool(listing.get("truncated"))
        said = f"{got} of 2100 entries listed, {'marked' if cut else 'not marked'} as cut"
        return ("PASS" if got == 2000 and cut else "FAIL"), said
    finally:
        adb("shell", device_command("rm", "-rf", where), timeout=300)


@check("y1-renamed", "§70")
def y1_renamed(ctx: Ctx) -> tuple[str, str]:
    """A file renamed between the listing and the fetch: the fetch says so,
    and nothing arrives under the old name."""
    found = shared_folder(ctx, writable=False)
    if not found:
        return "SKIP", "no read-only folder on the phone's storage in its Folders the computer may open"
    root, where, name = y1_place(found)

    def body(ctx: Ctx) -> tuple[str, str]:
        asked = time.time()
        ask_phone_files(ctx, "list", root, [name])
        first = wait_for(lambda: kept_listing(ctx, [name], asked), 60, 1)
        if not first:
            return "TIMEOUT", "no listing of the folder within 60 s"
        if first.get("error"):
            return "FAIL", f"the first listing said: {first['error']}"
        adb("shell", device_command("mv", f"{where}/f1.txt", f"{where}/g1.txt"))
        time.sleep(1)
        asked = time.time()
        ask_phone_files(ctx, "fetch", root, [name, "f1.txt"])
        # The phone answers with the folder's listing and its words for this
        # file; another error (a grant gone) is not this check's (Y1 review).
        said = wait_for(lambda: (l := kept_listing(ctx, [name], asked))
                        and "f1.txt is not on the phone any more" in (l.get("error") or "") and l, 30, 1)
        arrived = (ctx.server.root / "downloads" / "f1.txt").exists()
        if arrived:
            return "FAIL", "a file arrived under the old name"
        if not said:
            other = kept_listing(ctx, [name], asked)
            if other and other.get("error"):
                return "FAIL", f"the fetch said: {other['error']}"
            return "TIMEOUT", "no word on the fetch within 30 s"
        return "PASS", f"the fetch said: {said['error']}"

    try:
        y1_fill(where, 3)
        # A fetched file is sent as a share: by the internet road here.
        return road_check(ctx, body)
    finally:
        adb("shell", device_command("rm", "-rf", where), timeout=60)
        (ctx.server.root / "downloads" / "f1.txt").unlink(missing_ok=True)


# ---- the phone's sheets from UI dumps (Y3) ----


def dump_rows(dump: str) -> list[tuple[str, str, int, int]]:
    """Each view as `(text, content-desc, clickable 0/1, height in px)`."""
    out = []
    for node in re.findall(r"<node [^>]*>", dump):
        text = re.search(r' text="([^"]*)"', node)
        desc = re.search(r'content-desc="([^"]*)"', node)
        box = re.search(r'bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', node)
        if box:
            _, y1, _, y2 = map(int, box.groups())
            out.append((html.unescape(text.group(1)) if text else "", html.unescape(desc.group(1)) if desc else "",
                        int('clickable="true"' in node), y2 - y1))
    return out


def tap_text(label: str, dump: str) -> bool:
    """Tap the first view whose text or description is `label`."""
    spot = next(((x, y) for text, x, y in dump_nodes(dump) if text == label), None)
    if spot is None:
        found = re.search(r'<node [^>]*content-desc="' + re.escape(label) + r'"[^>]*bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', dump)
        if found:
            x1, y1, x2, y2 = map(int, found.groups())
            spot = ((x1 + x2) // 2, (y1 + y2) // 2)
    if spot is None:
        return False
    adb("shell", "input", "tap", str(spot[0]), str(spot[1]))
    return True


def density() -> float:
    found = re.search(r"(\d+)", adb("shell", "wm", "density").split("Override")[-1])
    return int(found.group(1)) / 160 if found else 3.0


def history_rows(dump: str) -> list[int]:
    """The heights of the clipboard history sheet's rows: the clickable
    views that carry a copy's text (their text is not read here)."""
    controls = {"Clear", "Cancel", "Clipboard history"}
    return [h for text, _, click, h in dump_rows(dump) if click and text and text not in controls]


@check("y3-history-sheet", "§125")
def y3_history_sheet(ctx: Ctx) -> tuple[str, str]:
    """The clipboard history sheet from a computer's card: rows 48 dp tall,
    Clear asking first (Cancel keeps every row), and with nothing kept, its
    words and no Clear. Rows are counted, never read; Clear is never
    confirmed."""
    ctx.phone.wake()
    adb("shell", "monkey", "-p", APP, "-c", "android.intent.category.LAUNCHER", "1")
    if not wait_for(lambda: tap_text("More for Test Desk", ctx.phone.dump()), 20, 2):
        return "SKIP", "no Test Desk card on Home"
    time.sleep(1)
    if not tap_text("Send from clipboard history", ctx.phone.dump()):
        # The card's menu closed again for the next check (Y3 review).
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        return "FAIL", "no Send from clipboard history in the card's menu"
    time.sleep(1.5)
    sheet = ctx.phone.dump()
    try:
        rows = history_rows(sheet)
        has_clear = any(r[0] == "Clear" for r in dump_rows(sheet))
        empty = any(r[0].startswith("Nothing copied yet") for r in dump_rows(sheet))
        if empty:
            return ("PASS" if not has_clear else "FAIL"), f"empty: its words, {'a Clear' if has_clear else 'no Clear'}"
        if not rows:
            return "FAIL", "neither rows nor the empty words in the sheet"
        # The last row may be cut by the screen's edge: not its height.
        full = rows[:-1] if len(rows) > 1 and rows[-1] < max(rows) else rows
        least = min(full) / density()
        if not tap_text("Clear", sheet):
            return "FAIL", f"{len(rows)} rows and no Clear"
        time.sleep(1)
        dialog = ctx.phone.dump()
        asked = "This deletes every saved clipboard entry from this phone." in dialog
        tap_text("Cancel", dialog)
        time.sleep(1)
        after = history_rows(ctx.phone.dump())
        said = f"{len(rows)} rows, at least {least:.0f} dp; Clear {'asked' if asked else 'did not ask'}; after Cancel {len(after)} rows"
        return ("PASS" if least >= 48 and asked and len(after) == len(rows) else "FAIL"), said
    finally:
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        # A dialog left open took that Back: the sheet goes too (Y3 review).
        time.sleep(1)
        if any(text == "Clipboard history" for text, _, _ in dump_nodes(ctx.phone.dump())):
            adb("shell", "input", "keyevent", "KEYCODE_BACK")


# ---- the clipboard's echo (Z1) ----


def pushes(log: str, device: str, length: int) -> int:
    """The server's "Clipboard sent to" lines for `device` of `length` chars."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    return sum(f"Clipboard sent to {device} ({length} chars)" in line for line in plain.splitlines())


@check("z1-echo", "§7")
def z1_echo(ctx: Ctx) -> tuple[str, str]:
    """No echo loop: a copy on the computer reaches the phone in a session
    once, and the phone, which then holds it, does not send it back."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        # The session's own first exchange settles first.
        time.sleep(8)
        token = "z1-" + secrets.token_hex(6)
        before = pushes(ctx.server.log(), ctx.device, len(token))
        adb("logcat", "-c")
        clip = ctx.server.root / "clip.txt"
        tmp = clip.with_suffix(".tmp")
        tmp.write_text(token, encoding="utf-8")
        os.replace(tmp, clip)
        if not wait_for(lambda: pushes(ctx.server.log(), ctx.device, len(token)) > before, 20, 1):
            return "TIMEOUT", "the server never sent the copy to the phone"
        time.sleep(20)
        sent = pushes(ctx.server.log(), ctx.device, len(token)) - before
        phone_log = adb("logcat", "-d", "-s", "PocketdeskRust:*", "PocketdeskClip:*").splitlines()
        back = sum(f"({len(token)} chars)" in line and "Clipboard sent to" in line for line in phone_log)
        # AA1: the phone says it took it, so "not sent back" means something.
        took = sum(f"text put on the clipboard ({len(token)} chars)" in line for line in phone_log)
        kept = ctx.server.clipboard() == token
        said = (f"sent to the phone {sent} time(s); the phone took it {took} time(s) and sent it back "
                f"{back} time(s); the computer's copy {'kept' if kept else 'changed'}")
        return ("PASS" if sent == 1 and took >= 1 and back == 0 and kept else "FAIL"), said
    return session_check(ctx, body)


# ---- shares from the phone, repeated (Z2) ----

# Sizes for z2-many-shares: small and large mixed, in this order.
Z2_SIZES = [1 << 10, 1 << 20, 50 << 20, 4 << 10, 200 << 20, 10 << 20, 100, 25 << 20]


@check("z2-many-shares", "§31")
def z2_many_shares(ctx: Ctx) -> tuple[str, str]:
    """Shares from the phone one after another, by the internet road: every
    one arrives whole. One that does not is named with the time it was
    shared, to be found in both logs (X1 saw one share never reach the
    computer)."""
    why = needs_choosing(ctx)
    if why:
        return "SKIP", why

    def body(ctx: Ctx) -> tuple[str, str]:
        lost = []
        arrived = 0
        for i, size in enumerate(Z2_SIZES):
            local = ctx.server.root / f"z2-{i}-{secrets.token_hex(3)}.bin"
            where = ctx.server.root / "downloads" / local.name
            try:
                # Inside the cleanup (Z2 review): a failed write leaves nothing.
                with open(local, "wb") as out:
                    out.write(os.urandom(size))
                # The staging starts here; the share follows it within seconds.
                at = time.strftime("%H:%M:%S")
                landed, why = shared(ctx, local, 120 + size // (1 << 20))
                if landed and md5_of(landed) == md5_of(local):
                    arrived += 1
                elif not landed and wait_for(lambda: where.exists() and where.stat().st_size == size, 60, 2):
                    # Slow, not lost: said so, and it does not run into the next.
                    lost.append(f"#{i} ({size} bytes, staged at {at}): late, past its deadline")
                else:
                    lost.append(f"#{i} ({size} bytes, staged at {at}): {why or 'not the same bytes'}")
            finally:
                local.unlink(missing_ok=True)
                where.unlink(missing_ok=True)
        said = f"{arrived} of {len(Z2_SIZES)} arrived whole"
        return ("PASS", said) if not lost else ("FAIL", said + "; lost: " + "; ".join(lost))
    return road_check(ctx, body)


LONG.add("z2-many-shares")


# ---- image copies without a session (AD1) ----


def made_png(seed: bytes) -> bytes:
    """A 16x16 RGB PNG whose pixels come from `seed`: a new image, and so a
    new id (its SHA-256), for each seed."""
    import hashlib
    import zlib

    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return len(data).to_bytes(4, "big") + body + zlib.crc32(body).to_bytes(4, "big")

    pixels = b""
    block = hashlib.sha256(seed).digest()
    while len(pixels) < 16 * 16 * 3:
        block = hashlib.sha256(block).digest()
        pixels += block
    rows = b"".join(b"\0" + pixels[r * 48:(r + 1) * 48] for r in range(16))
    header = (16).to_bytes(4, "big") * 2 + bytes([8, 2, 0, 0, 0])
    return (bytes([137, 80, 78, 71, 13, 10, 26, 10]) + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def cached_images(listing: str) -> set[str]:
    """The ids of the images in the phone's clipboard cache, from `ls`."""
    return {m.group(1) for m in re.finditer(r"^([0-9a-f]{64})\.png$", listing, re.MULTILINE)}


def images_told(log: str, device: str) -> int:
    """The server's "Clipboard image told to" lines for `device` (AD1)."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    return sum(f"Clipboard image told to {device} (" in line for line in plain.splitlines())


def phone_images() -> set[str]:
    return cached_images(adb("shell", device_command("run-as", APP, "ls", "cache/clipboard")))


def copy_image(ctx: Ctx, wait: float) -> tuple[bool, bool, bool]:
    """Put a new image on the throwaway's stand-in clipboard (the text
    stand-in emptied, as a real clipboard's text would win): whether the
    server told the phone of it, whether the phone fetched it, within
    `wait` seconds, and whether the phone said it put it on its clipboard
    (AH1, its size)."""
    import hashlib
    told = images_told(ctx.server.log(), ctx.device)
    adb("logcat", "-c")
    png = made_png(secrets.token_bytes(8))
    sha = hashlib.sha256(png).hexdigest()
    clip = ctx.server.root / "clip.txt"
    tmp = clip.with_suffix(".tmp")
    tmp.write_text("", encoding="utf-8")
    os.replace(tmp, clip)
    image = ctx.server.root / "clip.png"
    tmp = image.with_suffix(".tmp")
    tmp.write_bytes(png)
    os.replace(tmp, image)
    fetched = bool(wait_for(lambda: sha in phone_images(), wait, 3))
    line = f"image put on the clipboard ({len(png)} bytes)"
    said = bool(fetched and wait_for(lambda: line in adb("logcat", "-d", "-s", "PocketdeskClip:*"), 10, 1))
    return images_told(ctx.server.log(), ctx.device) > told, fetched, said


def restart_with_permission(ctx: Ctx, key: str, value: Optional[bool]) -> Optional[str]:
    """Restart the throwaway with the phone's permission `key` set (None
    takes it out), as the trust store holds it before the start, and the
    phone linked again by the internet road; why not, when it is not."""
    ctx.server.stop()
    path = ctx.server.root / "state" / "trusted_devices.json"
    store = json.loads(path.read_text(encoding="utf-8"))
    mine = store.setdefault("permissions", {}).setdefault(ctx.device, {})
    if value is None:
        mine.pop(key, None)
    else:
        mine[key] = value
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(store, indent=2), encoding="utf-8")
    os.replace(tmp, path)
    ctx.on_road = False
    why = by_road(ctx)
    if why:
        return why
    ctx.on_road = True
    return None


@check("ad1-image-copy", "§168")
def ad1_image_copy(ctx: Ctx) -> tuple[str, str]:
    """An image copied on the computer reaches the phone's clipboard without
    a session, with the phone's Computer's clipboard and Computer's copied
    images in the background on (§168); with the latter off, nothing is
    fetched. Then, with the phone's Clipboard permission off on the
    computer, nothing moves either (§38). By the internet road: the server
    brings the image to the phone by dialing it back, and never dials back
    to 127.0.0.1, which a phone through adb reverse is (its first runs
    passed or failed by which road the phone happened to be on)."""
    prefs = ctx.phone.prefs("pocketdesk-ui")
    if not prefs.get("clipboard_in_background"):
        return "SKIP", "turn on Computer's clipboard in the background in the phone's Settings"
    images_on = bool(prefs.get("clipboard_images_in_background"))
    path = ctx.server.root / "state" / "trusted_devices.json"
    had = json.loads(path.read_text(encoding="utf-8")).get("permissions", {}).get(ctx.device, {}).get("clipboard")
    # Read as each copy is pushed: no restart for it.
    before = ctx.server.configure({"clipboard_images": "true"})
    said = []
    changed = False
    try:
        if not ctx.on_road:
            why = by_road(ctx)
            if why:
                return "TIMEOUT", why
            ctx.on_road = True
        # The server's clipboard watch primes on its first tick with a phone
        # linked and never pushes what it primes on (review): a moment first.
        time.sleep(4)
        told, got, put = copy_image(ctx, 60 if images_on else 45)
        said.append(f"images in the background {'on' if images_on else 'off'}: "
                    f"{'told' if told else 'not told'}, {'fetched' if got else 'not fetched'}"
                    f"{', put on the clipboard' if put else ''}")
        # Told either way: what keeps it from the phone is the phone's switch.
        # Fetched, the phone says it put it on its clipboard (AH1).
        if not told or got != images_on or put != got:
            return "FAIL", "; ".join(said)
        if not images_on:
            return "PASS", "; ".join(said)
        changed = True
        why = restart_with_permission(ctx, "clipboard", False)
        if why:
            return "TIMEOUT", f"with the Clipboard permission off: {why}"
        time.sleep(4)
        told, got, _ = copy_image(ctx, 45)
        said.append(f"the Clipboard permission off: {'told' if told else 'not told'}, {'fetched' if got else 'not fetched'}")
        return ("FAIL" if told or got else "PASS"), "; ".join(said)
    finally:
        ctx.server.restore_config(before)
        (ctx.server.root / "clip.png").unlink(missing_ok=True)
        if changed:
            why = restart_with_permission(ctx, "clipboard", had)
            if why:
                print(f"note: putting the Clipboard permission back: {why}", file=sys.stderr)
# ---- the phone's remote file browser (AD2, AD3) ----
#
# The session's Files on this computer… lists the throwaway's own `home`
# (POCKETDESK_BROWSE_HOME), never the owner's. Its sheets are read from UI
# dumps; a tap is only ever on the phone's own controls, and the phone's
# input is off for the session checks anyway.

AD2_FILES = 2000


def tap_scrolling(ctx: Ctx, label: str, swipes: int = 8) -> bool:
    """Tap `label` in a sheet, scrolling it up until it shows."""
    for _ in range(swipes):
        if tap_text(label, ctx.phone.dump()):
            return True
        adb("shell", "input", "swipe", "540", "1900", "540", "1100", "300")
        time.sleep(1)
    return False


def shows(ctx: Ctx, label: str) -> bool:
    return any(text == label for text, _, _ in dump_nodes(ctx.phone.dump()))


def open_browser(ctx: Ctx) -> Optional[str]:
    """From a session, the browse sheet at home; why not, when it is not."""
    if not wait_for(lambda: tap_text("Session menu", ctx.phone.dump()), 20, 2):
        return "no Session menu on the session's screen"
    time.sleep(1)
    if not tap_scrolling(ctx, "Files on this computer…"):
        return "no Files on this computer… in the session menu"
    if not wait_for(lambda: shows(ctx, "Send a folder to this computer"), 20, 1):
        return "the browse sheet did not open"
    return None


def enter(ctx: Ctx, name: str) -> bool:
    """Open the folder `name` the sheet lists, and wait for its listing."""
    if not wait_for(lambda: tap_text(name, ctx.phone.dump()), 30, 2):
        return False
    return bool(wait_for(lambda: shows(ctx, "Up"), 30, 1))


def download_here(ctx: Ctx, what: str) -> bool:
    """Tap `what` ("Download this folder", or a file's name) and confirm."""
    if not tap_text(what, ctx.phone.dump()):
        return False
    return bool(wait_for(lambda: tap_text("Download", ctx.phone.dump()), 15, 1))


def close_sheets(ctx: Ctx) -> None:
    """Back out of the browse sheet, the window picker and the session
    menu, so the session ends from its own screen and the next check finds
    Home."""
    for _ in range(3):
        if not any(shows(ctx, title) for title in ("Send a folder to this computer", "Session settings", "Stream a window")):
            return
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        time.sleep(1)


def phone_names(folder: str) -> set[str]:
    """The files in `folder` of the phone's Pocketdesk downloads (a pending
    one, hidden, is not listed)."""
    return set(adb("shell", device_command("ls", f"{PHONE_DOWNLOADS}/{folder}"), timeout=60).split()) - {"ls:"}


def phone_count(folder: str) -> int:
    out = adb("shell", f"ls {PHONE_DOWNLOADS}/{folder} 2>/dev/null | wc -l", timeout=60).strip()
    return int(out) if out.isdigit() else 0


def settle_then_drop(ctx: Ctx, folder: str, settled: bool) -> None:
    """Remove `folder` from the phone once nothing more arrives in it: after
    a step that timed out, the server is stopped first and the phone's
    queued fetches allowed to end (AD2 review), or it would make it again."""
    if not settled:
        ctx.server.stop()
        ctx.on_road = False
    last, since, end = -1, time.monotonic(), time.monotonic() + 90
    while time.monotonic() < end and time.monotonic() - since < 15:
        now = phone_count(folder)
        if now != last:
            last, since = now, time.monotonic()
        time.sleep(3)
    drop_folder_from_phone(folder)


def drop_folder_from_phone(folder: str) -> None:
    # Only a name this runner made: letters, digits and dashes.
    if re.fullmatch(r"ad[23]-[a-z0-9-]+", folder):
        adb("shell", device_command("rm", "-rf", f"{PHONE_DOWNLOADS}/{folder}"), timeout=120)


@check("ad2-big-folder", "§52")
def ad2_big_folder(ctx: Ctx) -> tuple[str, str]:
    """A folder of thousands of files downloaded from the remote browser:
    the answer arrives, other folders still list while the offers come, and
    every file lands (§52)."""
    tag = secrets.token_hex(3)
    home = ctx.server.root / "home"
    many, other = f"ad2-many-{tag}", f"ad2-other-{tag}"
    settled = False

    def body(ctx: Ctx) -> tuple[str, str]:
        try:
            return steps(ctx)
        finally:
            close_sheets(ctx)

    def steps(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        why = open_browser(ctx)
        if why:
            return "TIMEOUT", why
        if not enter(ctx, many):
            return "TIMEOUT", f"the browser did not open {many}"
        asked = time.monotonic()
        if not download_here(ctx, "Download this folder"):
            return "TIMEOUT", "no Download this folder, or no Download to confirm it"
        # Another query while the folder's offers come.
        if not wait_for(lambda: tap_text("Up", ctx.phone.dump()), 15, 1):
            return "TIMEOUT", "no Up after asking for the folder"
        if not enter(ctx, other) or not wait_for(lambda: shows(ctx, "x.txt"), 30, 1):
            return "FAIL", "another folder did not list while the big one came"
        listed = time.monotonic() - asked
        # Overlapped only if the folder was still arriving (review).
        meanwhile = phone_count(many)
        if not wait_for(lambda: phone_count(many) > 0, 120, 3):
            return "TIMEOUT", "no file of the folder on the phone within 120 s"
        if not wait_for(lambda: phone_count(many) >= AD2_FILES, 2400, 10):
            return "TIMEOUT", f"{phone_count(many)} of {AD2_FILES} files on the phone after 40 minutes"
        took = time.monotonic() - asked
        nonlocal settled
        settled = True
        # Each file by its name (review): a retry's "f0001 (1).txt" must not
        # stand in for a file that never came.
        want = {f"f{i:04}.txt" for i in range(AD2_FILES)}
        got = phone_names(many)
        said = (f"{len(got & want)} of {AD2_FILES} files on the phone in {took:.0f} s;"
                f" another folder listed {listed:.0f} s after the ask, with {meanwhile} of them in")
        if got != want:
            return "FAIL", said + f"; missing {len(want - got)}, extra {len(got - want)}"
        if meanwhile >= AD2_FILES:
            return "SKIP", said + ": the folder had all come, nothing overlapped"
        return "PASS", said

    try:
        (home / many).mkdir(parents=True)
        (home / other).mkdir(parents=True)
        for i in range(AD2_FILES):
            (home / many / f"f{i:04}.txt").write_text(f"{i}" + NL, encoding="utf-8")
        (home / other / "x.txt").write_text("x" + NL, encoding="utf-8")
        return session_check(ctx, body)
    finally:
        shutil.rmtree(home / many, ignore_errors=True)
        shutil.rmtree(home / other, ignore_errors=True)
        settle_then_drop(ctx, many, settled)


LONG.add("ad2-big-folder")


@check("ad3-dates", "§149")
def ad3_dates(ctx: Ctx) -> tuple[str, str]:
    """A file and a folder downloaded from the remote browser keep their
    dates (§149), with both sides' Keep files' dates on (the defaults)."""
    if not ctx.phone.prefs("pocketdesk-ui").get("keep_file_dates", True):
        return "SKIP", "turn on Keep files' dates in the phone's Settings"
    tag = secrets.token_hex(3)
    home = ctx.server.root / "home"
    top, fold, name = f"ad3-{tag}", f"ad3-fold-{tag}", f"ad3-{tag}.txt"
    past = 1577968200  # 2020-01-02 12:30 UTC
    settled = False

    def phone_date(path: str) -> Optional[int]:
        out = adb("shell", device_command("stat", "-c", "%Y", f"{PHONE_DOWNLOADS}/{path}")).strip()
        return int(out) if out.isdigit() else None

    def body(ctx: Ctx) -> tuple[str, str]:
        try:
            return steps(ctx)
        finally:
            close_sheets(ctx)

    def steps(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        why = open_browser(ctx)
        if why:
            return "TIMEOUT", why
        if not enter(ctx, top):
            return "TIMEOUT", f"the browser did not open {top}"
        if not download_here(ctx, name):
            return "TIMEOUT", f"no Download for {name}"
        # The phone publishes a file, then dates it (review): its date read
        # until it is the one sent, the last read kept for a FAIL.
        seen = {}

        def dated(path: str) -> bool:
            seen[path] = phone_date(path)
            return seen[path] is not None and abs(seen[path] - past) <= 2

        wait_for(lambda: dated(name), 75, 2)
        file_date = seen.get(name)
        if not enter(ctx, fold) or not download_here(ctx, "Download this folder"):
            return "TIMEOUT", f"no Download this folder in {fold}"
        wait_for(lambda: dated(f"{fold}/inner.txt"), 105, 3)
        folder_date = seen.get(f"{fold}/inner.txt")
        nonlocal settled
        settled = True
        said = []
        for what, date in (("the file", file_date), ("the folder's file", folder_date)):
            if date is None:
                return "TIMEOUT", f"{what} did not reach the phone"
            kept = abs(date - past) <= 2
            said.append(f"{what} {'kept its date' if kept else 'dated ' + time.strftime('%Y-%m-%d', time.gmtime(date))}")
        kept = all(abs(d - past) <= 2 for d in (file_date, folder_date))
        return ("PASS" if kept else "FAIL"), "; ".join(said)

    try:
        (home / top / fold).mkdir(parents=True)
        for path in (home / top / name, home / top / fold / "inner.txt"):
            path.write_text("ad3" + NL, encoding="utf-8")
            os.utime(path, (past, past))
        return session_check(ctx, body)
    finally:
        shutil.rmtree(home / top, ignore_errors=True)
        settle_then_drop(ctx, fold, settled)
        drop_from_phone(name)


# ---- window crops in a session (AE1) ----
#
# The phone's window picker lists this computer's real windows. A row's
# second line is "<app>  ·  <W>x<H>[  ·  focused]": the runner reads the size
# alone from it and taps the row by position; a title or an app's name is
# never kept or printed.


def window_rows(dump: str) -> list[tuple[int, int, int, int]]:
    """`(width, height, x, y)` of each window the picker lists."""
    rows = []
    for text, x, y in dump_nodes(dump):
        found = re.search(r"  ·  (\d+)x(\d+)(?:  ·  |$)", text)
        if found:
            rows.append((int(found.group(1)), int(found.group(2)), x, y))
    return rows


def rebuilds(log: str) -> list[tuple[int, int, str]]:
    """`(width, height, codec)` of each "Encoder rebuilt at frame resolution"
    in the server's log (AE1 added the codec)."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    out = []
    for line in plain.splitlines():
        if "Encoder rebuilt at frame resolution" not in line:
            continue
        width, height = re.search(r"width=(\d+)", line), re.search(r"height=(\d+)", line)
        codec = re.search(r'codec="([^"]*)"', line) or re.search(r"codec=(\S+)", line)
        if width and height and codec:
            out.append((int(width.group(1)), int(height.group(1)), codec.group(1)))
    return out


def open_picker(ctx: Ctx) -> Optional[str]:
    """From a session, the window picker; why not, when it is not."""
    if not wait_for(lambda: tap_text("Session menu", ctx.phone.dump()), 20, 2):
        return "no Session menu on the session's screen"
    time.sleep(1)
    if not tap_scrolling(ctx, "Window"):
        return "no Window in the session menu"
    if not wait_for(lambda: shows(ctx, "Stream a window"), 20, 1):
        return "the window picker did not open"
    return None


def crop_to(ctx: Ctx, pick: Callable[[list[tuple[int, int, int, int]]], Optional[tuple[int, int, int, int]]]
            ) -> tuple[Optional[tuple[int, int]], str]:
    """Open the picker and tap the row `pick` chooses (None: Whole desktop);
    the size picked, or None and why not."""
    why = open_picker(ctx)
    if why:
        return None, why
    rows = wait_for(lambda: window_rows(ctx.phone.dump()), 20, 2) or []
    row = pick(rows)
    if row is None:
        return None, f"no window to crop to among {len(rows)}"
    adb("shell", "input", "tap", str(row[2]), str(row[3]))
    return (row[0], row[1]), ""


def capture_size(log: str) -> Optional[tuple[int, int]]:
    """The desktop's size, from the server's last "capture started" line."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    found = re.findall(r"capture started.*?width=(\d+) height=(\d+)", plain)
    return (int(found[-1][0]), int(found[-1][1])) if found else None


def a_window(rows: list[tuple[int, int, int, int]], desktop: Optional[tuple[int, int]]
             ) -> Optional[tuple[int, int, int, int]]:
    """The smallest window of at least 400x300 that is not the desktop's own
    size: its crop changes the frame's size."""
    fits = [r for r in rows if r[0] >= 400 and r[1] >= 300 and (r[0], r[1]) != desktop]
    return min(fits, key=lambda r: r[0] * r[1]) if fits else None


@check("ae1-crop-hevc", "§2")
def ae1_crop_hevc(ctx: Ctx) -> tuple[str, str]:
    """A crop to a window during an HEVC session: the encoder is rebuilt at
    the window's size and stays HEVC, the session stays up, and back to the
    whole desktop the same (R4 C1)."""
    def body(ctx: Ctx) -> tuple[str, str]:
        before = ctx.server.log().count("Client codec caps negotiated")
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        said = wait_for(lambda: ctx.server.log().count("Client codec caps negotiated") > before
                        and negotiated(ctx.server.log()), 30, 1)
        if not said:
            return "TIMEOUT", "no codec negotiated"
        if said[1] != "H.265 (HEVC)":
            return "SKIP", f"the session is {said[1]}: the phone cannot decode HEVC (caps {said[0]})"
        time.sleep(5)
        try:
            seen = len(rebuilds(ctx.server.log()))
            desktop = capture_size(ctx.server.log())
            if desktop is None:
                return "SKIP", "the server's log gives no desktop size to crop from"
            size, why = crop_to(ctx, lambda rows: a_window(rows, desktop))
            if size is None:
                return "SKIP", why
            # A rebuild away from the desktop's size (review: not some other).
            new = wait_for(lambda: [r for r in rebuilds(ctx.server.log())[seen:] if r[:2] != desktop], 30, 1)
            if not new:
                return "FAIL", f"no rebuild after a crop to {size[0]}x{size[1]}"
            cropped = new[-1]
            time.sleep(8)
            up = in_session(ctx)
            seen = len(rebuilds(ctx.server.log()))
            close_sheets(ctx)
            why = open_picker(ctx)
            if why:
                return "TIMEOUT", f"back to the whole desktop: {why}"
            if not tap_text("Whole desktop", ctx.phone.dump()):
                return "TIMEOUT", "no Whole desktop in the window picker"
            back = wait_for(lambda: [r for r in rebuilds(ctx.server.log())[seen:] if r[:2] == desktop], 30, 1)
            time.sleep(5)
            still = in_session(ctx)
        finally:
            close_sheets(ctx)
        said = (f"cropped to {size[0]}x{size[1]}: rebuilt at {cropped[0]}x{cropped[1]} {cropped[2]},"
                f" the session {'up' if up else 'gone'} 8 s on")
        if not back:
            return "FAIL", said + "; no rebuild back to the whole desktop"
        said += f"; whole again: {back[-1][0]}x{back[-1][1]} {back[-1][2]}, the session {'up' if still else 'gone'}"
        hevc = cropped[2] == "H.265 (HEVC)" and back[-1][2] == "H.265 (HEVC)"
        return ("PASS" if hevc and up and still else "FAIL"), said

    before = ctx.server.configure({"allow_hevc": "true"})
    try:
        # Read at the server's start: the road again, from a restart.
        ctx.on_road = False
        return session_check(ctx, body)
    finally:
        ctx.server.restore_config(before)
        # The next session check starts the server again, without HEVC.
        ctx.on_road = False


# ---- a software-fallback session stays on software across crops (AM1) ----


def fallbacks(log: str) -> int:
    """The server's "Hardware encoder unavailable" lines: each a hardware
    rung tried and given up (C2)."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    return sum("Hardware encoder unavailable" in line for line in plain.splitlines())


@check("am1-sticky-software", "§2")
def am1_sticky_software(ctx: Ctx) -> tuple[str, str]:
    """A session whose hardware encoder failed as it opened goes on in
    software, and a crop to a window and back rebuilds the encoder there
    without trying the dead hardware again (§2, R4 C2: the encoder it fell
    back to is the one each rebuild starts from). The throwaway is started
    with every hardware rung failing as it opens (`POCKETDESK_FAIL_ENCODERS`,
    a test seam), as one whose probe passed but whose encoder dies at
    startup does."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if "POCKETDESK_FAIL_ENCODERS is set" not in re.sub(chr(27) + r"\[[0-9;]*m", "", ctx.server.log()):
            return "SKIP", "the throwaway did not say its encoder seam was set"
        records = len(session_records(ctx.server.root))
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: fallbacks(ctx.server.log()) and in_session(ctx), 40, 1):
            return "TIMEOUT", "no session that fell back to software within 40 s"
        time.sleep(5)
        try:
            opened = fallbacks(ctx.server.log())
            seen = len(rebuilds(ctx.server.log()))
            desktop = capture_size(ctx.server.log())
            if desktop is None:
                return "SKIP", "the server's log gives no desktop size to crop from"
            size, why = crop_to(ctx, lambda rows: a_window(rows, desktop))
            if size is None:
                return "SKIP", why
            new = wait_for(lambda: [r for r in rebuilds(ctx.server.log())[seen:] if r[:2] != desktop], 30, 1)
            if not new:
                return "FAIL", f"no rebuild after a crop to {size[0]}x{size[1]}"
            time.sleep(5)
            seen = len(rebuilds(ctx.server.log()))
            close_sheets(ctx)
            why = open_picker(ctx)
            if why:
                return "TIMEOUT", f"back to the whole desktop: {why}"
            if not tap_text("Whole desktop", ctx.phone.dump()):
                return "TIMEOUT", "no Whole desktop in the window picker"
            back = wait_for(lambda: [r for r in rebuilds(ctx.server.log())[seen:] if r[:2] == desktop], 30, 1)
            time.sleep(5)
            up = in_session(ctx)
            again = fallbacks(ctx.server.log()) - opened
        finally:
            close_sheets(ctx)
        end_session(ctx)
        ended = wait_for(lambda: session_records(ctx.server.root)[records:], 30, 1)
        encoder = ended[-1].get("encoder", "?") if ended else "no record"
        said = (f"the hardware given up {opened} time(s) at the start; cropped to {size[0]}x{size[1]}"
                f" and {'back' if back else 'not back'}: given up {again} more time(s); the session"
                f" {'up' if up else 'gone'}; its record's encoder {encoder}")
        # The software rung's names: the sidecar, or in-process on Linux (review).
        ok = back and again == 0 and up and encoder in ("ffmpeg-sidecar", "in-process")
        return ("PASS" if ok else "FAIL"), said

    ctx.server.extra = {"POCKETDESK_FAIL_ENCODERS": "nvenc,amf,qsv,mf,vaapi"}
    try:
        # Read at the server's start: the road again, from a restart.
        ctx.on_road = False
        return session_check(ctx, body)
    finally:
        ctx.server.extra = {}
        # The next session check starts the server again, without the seam.
        ctx.on_road = False


def session_records(root: pathlib.Path) -> list[dict[str, str]]:
    """The throwaway's session records (`streaming_sessions.log`), each a
    line of tab-separated `key=value` fields, as dicts."""
    path = root / "state" / "streaming_sessions.log"
    if not path.exists():
        return []
    return [dict(f.split("=", 1) for f in line.split(chr(9)) if "=" in f)
            for line in path.read_text(encoding="utf-8", errors="replace").splitlines() if line.strip()]


AE2_CROPS = 6


@check("ae2-crops", "§16")
def ae2_crops(ctx: Ctx) -> tuple[str, str]:
    """Crops to a window and back, six times in one session: each rebuilds
    at its size, the session stays up with no redial (no watchdog), the
    app's memory level holds (§16, across resolution changes). The
    session's record is read after: its rebuild tail (AH2) times each
    change of size, the stall a crop makes (§11), which the frames' encode
    tail leaves out."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        time.sleep(10)
        sessions = ctx.server.log().count("Client codec caps negotiated")
        if sessions < 1:
            return "TIMEOUT", "no codec negotiated for the session: a redial could not be seen"
        records = len(session_records(ctx.server.root))
        desktop = capture_size(ctx.server.log())
        if desktop is None:
            return "SKIP", "the server's log gives no desktop size to crop from"
        memory = [pss_mb()]
        sizes = []
        try:
            for _ in range(AE2_CROPS):
                for whole in (False, True):
                    seen = len(rebuilds(ctx.server.log()))
                    if whole:
                        why = open_picker(ctx)
                        if why:
                            return "TIMEOUT", why
                        if not tap_text("Whole desktop", ctx.phone.dump()):
                            return "TIMEOUT", "no Whole desktop in the window picker"
                    else:
                        size, why = crop_to(ctx, lambda rows: a_window(rows, desktop))
                        if size is None:
                            return "SKIP", why
                    # The rebuild this change makes (AE1 review): at the
                    # desktop's size going back, away from it cropping.
                    new = wait_for(lambda: [r for r in rebuilds(ctx.server.log())[seen:]
                                            if (r[:2] == desktop) == whole], 30, 1)
                    if not new:
                        return "FAIL", f"no rebuild after crop {len(sizes) + 1}"
                    sizes.append(f"{new[-1][0]}x{new[-1][1]}")
                    close_sheets(ctx)
                    time.sleep(5)
                    if not in_session(ctx):
                        # The phone's side, kept at once (AI2): by the run's
                        # end its log has moved past the moment.
                        kept = ctx.server.root / f"phone_drop_{int(time.time())}.txt"
                        kept.write_text(adb("logcat", "-d", "-v", "time", timeout=120), encoding="utf-8")
                        return "FAIL", f"the session ended after {len(sizes)} changes of size (the phone's log kept in {kept.name})"
                memory.append(pss_mb())
        finally:
            close_sheets(ctx)
        # A redial shows as a new negotiation, or as a session's record
        # written before this one ends (review: not every redial negotiates).
        redials = max(ctx.server.log().count("Client codec caps negotiated") - sessions,
                      len(session_records(ctx.server.root)) - records)
        end_session(ctx)
        record = wait_for(lambda: session_records(ctx.server.root)[records:], 30, 1)
        tail = record[-1] if record else {}
        if memory[0] is None or memory[-1] is None:
            return "TIMEOUT", "the app's memory level could not be read at the start or the end"
        levels = [m for m in memory if m is not None]
        grew = levels[-1] - levels[0]
        peak = max(levels) - levels[0]
        said = (f"{len(sizes)} changes of size ({', '.join(sorted(set(sizes)))}); redials {redials};"
                f" memory {' → '.join(f'{m:.1f}' for m in levels)} MB;"
                f" record ({tail.get('outcome', '?')}): enc p50 {tail.get('enc_p50', '?')} max {tail.get('enc_max', '?')},"
                f" rtt p50 {tail.get('rtt_p50', '?')} max {tail.get('rtt_max', '?')} ms;"
                f" rebuilds {tail.get('rebuild_n', 'none')}, p50 {tail.get('rebuild_p50', '?')} max {tail.get('rebuild_max', '?')} ms")
        if not tail:
            return "TIMEOUT", said + "; no session record after the session ended"
        # A leak shows as a climb that keeps on: more than 40 MB over six
        # rounds (at the end or at any round), where a round's rebuilds are a
        # few MB and come back.
        good = tail.get("outcome") in ("completed", "lan_direct", "wan_punched", "wan_relayed")
        # Each change of size rebuilt the encoder, and each is in the record.
        timed = tail.get("rebuild_n", "").isdigit() and int(tail["rebuild_n"]) >= len(sizes)
        ok = redials == 0 and grew <= 40 and peak <= 40 and good and timed
        return ("PASS" if ok else "FAIL"), said
    return session_check(ctx, body)


LONG.add("ae2-crops")


# ---- Quality presets changed mid-session (AN1) ----


def preset_changes(log: str) -> int:
    """The server's "Link-profile preset changed" lines (R4 E5)."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    return sum("Link-profile preset changed" in line for line in plain.splitlines())


def tap_after(dump: str, row: str, choice: str) -> bool:
    """Tap the first `choice` after the label `row` in the dump: "Auto" is
    a choice of more than one row of the session menu."""
    nodes = dump_nodes(dump)
    at = next((i for i, (text, _, _) in enumerate(nodes) if text == row), None)
    if at is None:
        return False
    spot = next(((x, y) for text, x, y in nodes[at + 1:] if text == choice), None)
    if spot is None:
        return False
    adb("shell", "input", "tap", str(spot[0]), str(spot[1]))
    return True


@check("an1-presets", "§11b")
def an1_presets(ctx: Ctx) -> tuple[str, str]:
    """The Quality preset changed mid-session from the session menu,
    Economy, Max, Balanced and back to Auto: the server logs each change,
    the session stays up with no redial, ends well, and any rebuild in its
    record stays under a second (§11b, R4 E5). The preset is control, never
    injected: it reaches the server with the phone's input off."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        records = len(session_records(ctx.server.root))
        # Counted once the stream is set up, as ae2-crops does (review).
        time.sleep(10)
        sessions = ctx.server.log().count("Client codec caps negotiated")
        if sessions < 1:
            return "TIMEOUT", "no codec negotiated for the session: a redial could not be seen"
        before = preset_changes(ctx.server.log())
        taken = []
        try:
            if not wait_for(lambda: tap_text("Session menu", ctx.phone.dump()), 20, 2):
                return "TIMEOUT", "no Session menu on the session's screen"
            time.sleep(1)
            if not tap_scrolling(ctx, "Quality"):
                return "TIMEOUT", "no Quality in the session menu"
            for choice in ("Economy", "Max", "Balanced", "Auto"):
                if not wait_for(lambda: tap_after(ctx.phone.dump(), "Quality", choice), 10, 1):
                    return "FAIL", f"no {choice} on the Quality row (after {', '.join(taken) or 'none'})"
                logged = wait_for(lambda: preset_changes(ctx.server.log()) > before + len(taken), 10, 0.5)
                taken.append(choice if logged else f"{choice} (not logged)")
                # A few seconds at each: long enough for a rebuild to show.
                time.sleep(6)
                if not in_session(ctx):
                    break
        finally:
            close_sheets(ctx)
        up = in_session(ctx)
        redials = max(ctx.server.log().count("Client codec caps negotiated") - sessions,
                      len(session_records(ctx.server.root)) - records)
        end_session(ctx)
        record = wait_for(lambda: session_records(ctx.server.root)[records:], 30, 1)
        tail = record[-1] if record else {}
        longest = tail.get("rebuild_max", "")
        said = (f"{', '.join(taken)}; {preset_changes(ctx.server.log()) - before} changes logged;"
                f" the session {'up' if up else 'gone'}, redials {redials};"
                f" rebuilds {tail.get('rebuild_n', 'none')}, max {longest or '?'} ms")
        if not record:
            return "TIMEOUT", said + "; no session record after the session ended"
        # Each change logged as it was made, not made up by a stray (review).
        logged_all = not any(t.endswith("(not logged)") for t in taken) and len(taken) == 4
        quick = not longest or (longest.isdigit() and int(longest) < 1000)
        good = tail.get("outcome") in ("completed", "lan_direct", "wan_punched", "wan_relayed")
        ok = logged_all and up and redials == 0 and quick and good
        return ("PASS" if ok else "FAIL"), said
    return session_check(ctx, body)


# ---- a download from the remote browser cut and continued (AF1) ----


def phone_size(path: str) -> Optional[int]:
    out = adb("shell", device_command("stat", "-c", "%s", f"{PHONE_DOWNLOADS}/{path}")).strip()
    return int(out) if out.isdigit() else None


def transfers_state() -> Optional[bool]:
    """Whether the phone's TransferService runs; None when dumpsys did not
    answer as it does (AF2 review: a failed adb call is not "ended")."""
    out = adb("shell", "dumpsys", "activity", "services", f"{APP}/.service.TransferService")
    if "ServiceRecord" in out:
        return True
    return False if "ACTIVITY MANAGER SERVICES" in out else None


def browse_download(ctx: Ctx, folder: str, name: str) -> Optional[str]:
    """In a new session, ask the remote browser for `folder`/`name`; why not,
    when it could not."""
    if start_session(ctx) is None:
        return "no Connect on a 127.0.0.1 card on Home"
    if not wait_for(lambda: in_session(ctx), 30, 1):
        return "no session within 30 s"
    try:
        why = open_browser(ctx)
        if why:
            return why
        if not enter(ctx, folder):
            return f"the browser did not open {folder}"
        if not download_here(ctx, name):
            return f"no Download for {name}"
        return None
    finally:
        close_sheets(ctx)


@check("af1-browse-resume", "§50")
def af1_browse_resume(ctx: Ctx) -> tuple[str, str]:
    """A file from the remote browser broken and continued: a 1 GiB download
    cut at 30 % (the throwaway stopped, as a dropped network would)
    continues from what the phone kept, by its own retry or asked again,
    and ends with the same MD5 (§50)."""
    if not ctx.phone.prefs("pocketdesk-ui").get("retry_transfers", True):
        return "SKIP", "turn on Retry failed files in the phone's Settings"
    tag = secrets.token_hex(3)
    home = ctx.server.root / "home"
    folder, name = f"af1-{tag}", f"af1-{tag}.bin"
    settled = False

    def body(ctx: Ctx) -> tuple[str, str]:
        nonlocal settled
        path = home / folder / name
        want = md5_of(path)
        size = path.stat().st_size
        adb("logcat", "-c")
        why = browse_download(ctx, folder, name)
        if why:
            return "TIMEOUT", why
        part = wait_for(lambda: (d := transfer_done(ctx, name)) and d[1] and d[0] >= d[1] * 0.3 and d, 600, 1)
        if not part:
            return "TIMEOUT", "the download never reached 30 %"
        ctx.server.stop()
        ctx.on_road = False
        if phone_size(name) == size:
            # AF1 review: a download that ended before the stop was never cut.
            return "TIMEOUT", "the download finished before the cut"
        why = by_road(ctx)
        if why:
            return "TIMEOUT", f"after the cut: {why}"
        ctx.on_road = True
        said = ""
        # The phone's own retry first (5, 15, 45 s apart; the road can rest
        # longer), then asked again from the browser once it stands still.
        how = "the phone's retry"
        last, since, end = -1, time.monotonic(), time.monotonic() + 1500
        while phone_size(name) != size:
            if time.monotonic() > end:
                return "TIMEOUT", f"cut at {part[0] * 100 // part[1]} %; not whole on the phone within 25 minutes ({how})"
            now = (transfer_done(ctx, name) or (0, 0))[0]
            if now != last:
                last, since = now, time.monotonic()
            elif time.monotonic() - since > 120 and how != "asked again" and transfers_state() is False:
                # Only once the phone's retries have ended (review: a re-ask
                # over a slow retry downloads the file twice). The log first:
                # a new session's start clears it.
                said += adb("logcat", "-d", "-s", "PocketdeskRust:*")
                how = "asked again"
                end_session(ctx)
                why = browse_download(ctx, folder, name)
                if why:
                    return "TIMEOUT", f"asking again: {why}"
                since = time.monotonic()
            time.sleep(3)
        settled = True
        said += adb("logcat", "-d", "-s", "PocketdeskRust:*")
        # This file's: its size (review).
        found = next((m for m in re.finditer(r"Download continued from (\d+) of (\d+) bytes", said)
                      if int(m.group(2)) == size), None)
        got = adb("shell", device_command("md5sum", f"{PHONE_DOWNLOADS}/{name}"), timeout=300).split()
        if got[:1] == ["TIMEOUT"]:
            return "TIMEOUT", "md5sum on the phone ran past 300 s"
        same = bool(got) and got[0] == want
        cut = f"cut at {part[0] * 100 // part[1]} %; {how}"
        if not found:
            return "FAIL", f"{cut} started over (no \"continued\" in the phone's log), MD5 {'matches' if same else 'differs'}"
        offset = int(found.group(1))
        return ("PASS" if same and offset > 0 else "FAIL"), (
            f"{cut} continued from {offset * 100 // int(found.group(2))} %, MD5 {'matches' if same else 'differs'}")

    try:
        (home / folder).mkdir(parents=True)
        with open(home / folder / name, "wb") as out:
            left = GIB
            while left:
                chunk = min(left, 8 << 20)
                out.write(os.urandom(chunk))
                left -= chunk
        return session_check(ctx, body)
    finally:
        if not settled:
            # A download may still run: stopped first (AD2 review), which also
            # lets go of the file.
            ctx.server.stop()
            ctx.on_road = False
        shutil.rmtree(home / folder, ignore_errors=True)
        drop_from_phone(name)


LONG.add("af1-browse-resume")


# ---- the phone's Retry failed files off (AF2) ----


@check("af2-retry-off", "§50")
def af2_retry_off(ctx: Ctx) -> tuple[str, str]:
    """With the phone's Retry failed files off, a send cut part way fails at
    once (no retries, which would take a minute and more), and nothing of
    it is kept on the phone (§50)."""
    if ctx.phone.prefs("pocketdesk-ui").get("retry_transfers", True):
        return "SKIP", "turn off Retry failed files in the phone's Settings (this checks it off)"

    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        path = big_file(ctx, GIB)
        stopped = False
        try:
            first = send_cli(ctx, path, None)
            try:
                first.communicate(timeout=60)
            except subprocess.TimeoutExpired:
                first.kill()
            part = wait_for(lambda: (d := transfer_done(ctx, path.name)) and d[1] and d[0] >= d[1] * 0.3 and d, 600, 1)
            if not part:
                return "TIMEOUT", "the send never reached 30 %"
            if transfers_state() is not True:
                return "TIMEOUT", "the phone's transfer service was not seen running at the cut"
            # The cut; the server stays down, so a retry could only fail.
            ctx.server.stop()
            ctx.on_road = False
            stopped = True
            cut = time.monotonic()
            # Ended: seen gone twice running, never a failed adb call (review).
            gone = []
            ended = wait_for(lambda: gone.append(transfers_state() is False) or gone[-2:] == [True, True], 180, 1)
            took = time.monotonic() - cut
            time.sleep(5)
            kept = on_phone(path.name)
            said = (f"cut at {part[0] * 100 // part[1]} %; the phone's transfer ended "
                    f"{f'{took:.0f} s after the cut' if ended else 'not within 180 s'}; "
                    f"{len(kept)} entr{'y' if len(kept) == 1 else 'ies'} of it kept on the phone")
            # The phone needs about 30 s to call the link dead (its first run:
            # 29 s); retries would wait 5, 15 and 45 s more between attempts.
            # Ending within 60 s is ending at once, with no retry.
            return ("PASS" if ended and took <= 60 and not kept else "FAIL"), said
        finally:
            if not stopped:
                # A send may still run, and hold the file: stopped first (review).
                ctx.server.stop()
                ctx.on_road = False
            drop_from_phone(path.name)
            path.unlink(missing_ok=True)
    return session_check(ctx, body)


LONG.add("af2-retry-off")


# ---- the mic share's permission refused (AF3) ----


def switch_on_row(dump: str, label: str) -> Optional[bool]:
    """Whether the switch on the row of `label` is on; None when the row or
    its switch is not shown."""
    nodes = []
    for node in re.findall(r"<node [^>]*>", dump):
        text = re.search(r' text="([^"]*)"', node)
        box = re.search(r'bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', node)
        if text and box:
            x1, y1, x2, y2 = map(int, box.groups())
            nodes.append((html.unescape(text.group(1)), 'checkable="true"' in node,
                          'checked="true"' in node, (y1 + y2) // 2))
    row = next((y for text, _, _, y in nodes if text == label), None)
    if row is None:
        return None
    near = [(abs(y - row), checked) for _, checkable, checked, y in nodes if checkable and abs(y - row) <= 60]
    return min(near)[1] if near else None


def mic_permission() -> str:
    """The phone's RECORD_AUDIO line for Pocketdesk, from dumpsys."""
    out = adb("shell", "dumpsys", "package", APP)
    return next((line.strip() for line in out.splitlines() if "android.permission.RECORD_AUDIO: granted=" in line), "")


def mic_granted() -> bool:
    return "granted=true" in mic_permission()


@check("af3-mic-refused", "§10a")
def af3_mic_refused(ctx: Ctx) -> tuple[str, str]:
    """The session's Phone mic share without the mic permission asks for it;
    refused, the switch stays off (§10a). One denial marks the permission
    (USER_SET), and the OPPO's shell may not clear the mark; a second would
    make Android stop asking (USER_FIXED), so the check runs once and SKIPs
    on a marked permission. It also SKIPs with the permission granted:
    taking it back would end the app."""
    held = mic_permission()
    if not held:
        # Unread is not unmarked (review): a second refusal cannot be undone.
        return "SKIP", "the mic permission could not be read"
    if "granted=true" in held:
        return "SKIP", "Pocketdesk holds the mic permission: taking it back would end the app"
    if "USER_SET" in held or "USER_FIXED" in held:
        return "SKIP", "the mic permission was refused once already: a second refusal would stop Android asking"

    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        time.sleep(4)
        try:
            if not wait_for(lambda: tap_text("Session menu", ctx.phone.dump()), 20, 2):
                return "TIMEOUT", "no Session menu on the session's screen"
            time.sleep(1)
            if not tap_scrolling(ctx, "Phone mic share"):
                return "TIMEOUT", "no Phone mic share in the session menu"
            # Android may write it with a typographic apostrophe (review).
            deny = wait_for(lambda: next((t for t, _, _ in dump_nodes(ctx.phone.dump())
                                          if re.fullmatch(r"Don.t allow", t)), None), 15, 1)
            if not deny:
                return "FAIL", "no permission dialog after Phone mic share"
            tap_text(deny, ctx.phone.dump())
            time.sleep(2)
            on = wait_for(lambda: switch_on_row(ctx.phone.dump(), "Phone mic share") is not None
                          and switch_on_row(ctx.phone.dump(), "Phone mic share"), 5, 1)
            state = switch_on_row(ctx.phone.dump(), "Phone mic share")
        finally:
            close_sheets(ctx)
        granted = mic_granted()
        said = f"asked; refused: the switch {'on' if state else 'off' if state is False else 'not seen'}, the permission {'granted' if granted else 'not granted'}"
        return ("PASS" if state is False and not on and not granted else "FAIL"), said

    return session_check(ctx, body)


# ---- a sound session left without its computer (AF4) ----


def volume_of(said: str) -> Optional[int]:
    """A stream's volume from `cmd media_session volume --get`."""
    found = re.search(r"volume is (\d+) in range", said)
    return int(found.group(1)) if found else None


def media_volume() -> Optional[int]:
    return volume_of(adb("shell", "cmd", "media_session", "volume", "--stream", "3", "--get"))


def session_service_state() -> Optional[bool]:
    """Whether the phone's SessionForegroundService runs; None when dumpsys
    did not answer as it does."""
    out = adb("shell", "dumpsys", "activity", "services", f"{APP}/.service.SessionForegroundService")
    if "ServiceRecord" in out:
        return True
    return False if "ACTIVITY MANAGER SERVICES" in out else None


@check("af4-sound-gives-up", "§97")
def af4_sound_gives_up(ctx: Ctx) -> tuple[str, str]:
    """A sound session (the phone as the computer's speaker and mic) left
    without its computer tries again for ten minutes, then stops trying and
    releases its service (§97). The phone's media volume is held at 0 for
    the run (it is night in the owner's home) and put back after."""
    if not ctx.phone.prefs("pocketdesk-ui").get("auto_reconnect", True):
        return "SKIP", "turn on reconnecting in the phone's Settings"

    def body(ctx: Ctx) -> tuple[str, str]:
        volume = media_volume()
        if volume is None:
            return "SKIP", "the phone's media volume could not be read, so it could not be held at 0"
        if volume:
            adb("shell", "cmd", "media_session", "volume", "--stream", "3", "--set", "0")
        try:
            ctx.phone.wake()
            adb("shell", "monkey", "-p", APP, "-c", "android.intent.category.LAUNCHER", "1")
            if not wait_for(lambda: tap_text("More for Test Desk", ctx.phone.dump()), 20, 2):
                return "SKIP", "no More for Test Desk on Home"
            if not wait_for(lambda: tap_text("Sound", ctx.phone.dump()), 10, 1):
                return "TIMEOUT", "no Sound in the card's menu"
            if not wait_for(lambda: session_service_state() is True, 60, 2):
                return "TIMEOUT", "the sound session's service did not start"
            # Admitted by the computer, not only started on the phone (review:
            # the service starts before the dial).
            if not wait_for(lambda: in_session(ctx), 60, 2):
                return "TIMEOUT", "the computer never admitted the sound session"
            time.sleep(15)
            # The computer goes away and stays away.
            ctx.server.stop()
            ctx.on_road = False
            cut = time.monotonic()
            seen = []
            ended = wait_for(lambda: seen.append(session_service_state() is False) or seen[-2:] == [True, True],
                             15 * 60, 10)
            took = (time.monotonic() - cut) / 60
            said = (f"the computer stopped; the service {'released' if ended else 'still held'} "
                    f"{f'{took:.1f} min after' if ended else 'after 15 min'}")
            # Ten minutes of tries with backoff (2, 4, 8, 16, then 30 s
            # apart), given up after a failed try: released between 9 and 13.
            return ("PASS" if ended and 9 <= took <= 13 else "FAIL"), said
        finally:
            # Exit first, the screen woken (it is off by now), and only then
            # the volume back (review: never the computer's sound out loud).
            ctx.phone.wake()
            tap_text("Exit", ctx.phone.dump())
            if volume:
                adb("shell", "cmd", "media_session", "volume", "--stream", "3", "--set", str(volume))

    return road_check(ctx, body)


LONG.add("af4-sound-gives-up")


# ---- a failed away lock (AG1) ----


def by_home(ctx: Ctx) -> Optional[str]:
    """The server restarted with the reverse, the phone linked at home (not
    by the internet road); why not, when it is not."""
    ctx.server.stop()
    why = ctx.server.start()
    if why:
        return why
    adb("reverse", f"tcp:{ctx.phone_port}", f"tcp:{ctx.port}")
    ctx.phone.wake()
    if not wait_for(lambda: ctx.device in ctx.server.linked(), 90, 2):
        return "the phone did not link at home within 90 s"
    if iroh_loops(ctx.server.log()):
        return "the phone linked by the internet road, not at home"
    ctx.on_road = False
    return None


def lock_failures(log: str, device: str) -> int:
    """The server's "left, but the lock failed" lines for `device`."""
    plain = re.sub(chr(27) + r"\[[0-9;]*m", "", log)
    return sum(f"{device} left, but the lock failed" in line for line in plain.splitlines())


def set_permission_live(ctx: Ctx, key: str, value: Optional[bool]) -> None:
    """The phone's permission `key` in the trust store, while the server
    runs (the away lock's switch is read at each tick); None takes it out."""
    path = ctx.server.root / "state" / "trusted_devices.json"
    store = json.loads(path.read_text(encoding="utf-8"))
    mine = store.setdefault("permissions", {}).setdefault(ctx.device, {})
    if value is None:
        mine.pop(key, None)
    else:
        mine[key] = value
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(store, indent=2), encoding="utf-8")
    os.replace(tmp, path)


@check("ag1-failed-lock", "§118")
def ag1_failed_lock(ctx: Ctx) -> tuple[str, str]:
    """A lock that fails is tried again a minute later, and said in an
    `away_locked` event with `failed` (§118). The lock is the throwaway's
    seam file, never this computer's: made read-only, it fails; writable
    again, the next try writes its line. The phone counts as present at
    home (the reverse) and as gone on the internet road alone (V1)."""
    lock = ctx.server.root / "lock.txt"
    path = ctx.server.root / "state" / "trusted_devices.json"
    had = json.loads(path.read_text(encoding="utf-8")).get("permissions", {}).get(ctx.device, {}).get("away_lock")
    before = None
    switched = False
    if os.environ.get("ANDROID_SERIAL", "").count(":"):
        # Restarting adb's server would lose a phone on wireless adb (review).
        return "SKIP", "the phone is on wireless adb: adb's server cannot be restarted under it"
    try:
        # A run starts at home; only a check before this one moved it to the
        # road (a phone that found home unreachable may stay on the road:
        # then run this check alone, or first).
        if ctx.on_road or iroh_loops(ctx.server.log()):
            why = by_home(ctx)
            if why:
                return "SKIP", f"{why}: run this check alone"
        # The seam is set for this server (its own start says so), or the
        # switch is never turned on: nothing may lock this computer.
        if f"POCKETDESK_LOCK_FILE is set: {lock}" not in re.sub(chr(27) + r"\[[0-9;]*m", "", ctx.server.log()):
            return "SKIP", "the throwaway did not say its lock seam was set: the away lock stays off"
        if lock.exists():
            # A run cut short may have left it read-only (review).
            os.chmod(lock, stat.S_IREAD | stat.S_IWRITE)
        lock.unlink(missing_ok=True)
        lock.write_text("", encoding="utf-8")
        os.chmod(lock, stat.S_IREAD)
        before = ctx.server.configure({"away_lock_minutes": "2", "events": "true"})
        count = ctx.server.event_count()
        failures = lock_failures(ctx.server.log(), ctx.device)
        set_permission_live(ctx, "away_lock", True)
        switched = True
        time.sleep(20)
        # Gone: the reverse removed, and adb's own server restarted, as that
        # alone closes the link already made through the reverse (its first
        # run kept it); the phone comes back by the internet road alone.
        loops = iroh_loops(ctx.server.log())
        adb("reverse", "--remove", f"tcp:{ctx.phone_port}")
        run(["adb", "kill-server"])
        run(["adb", "start-server"], timeout=60)
        run(["adb", "wait-for-device"], timeout=60)
        if not wait_for(lambda: iroh_loops(ctx.server.log()) > loops, 240, 3):
            return "TIMEOUT", "the phone did not come back by the internet road within 240 s"
        ctx.on_road = True
        gone = time.monotonic()
        first = wait_for(lambda: lock_failures(ctx.server.log(), ctx.device) >= failures + 1, 8 * 60, 5)
        if not first:
            return "TIMEOUT", "no lock tried within 8 minutes of the phone leaving (is this computer in use?)"
        tried = (time.monotonic() - gone) / 60
        failed_event = wait_for(lambda: any(e.get("event") == "away_locked" and e.get("failed")
                                            for e in ctx.server.events_after_count(count)), 30, 2)
        again_at = time.monotonic()
        second = wait_for(lambda: lock_failures(ctx.server.log(), ctx.device) >= failures + 2, 180, 5)
        again = time.monotonic() - again_at
        # Writable again: the next try writes its line.
        os.chmod(lock, stat.S_IREAD | stat.S_IWRITE)
        written = wait_for(lambda: lock.read_text(encoding="utf-8").strip(), 180, 5)
        said = (f"the phone gone, the lock tried {tried:.1f} min later and failed;"
                f" {'an' if failed_event else 'no'} away_locked event with failed;"
                f" {f'tried again {again:.0f} s later' if second else 'not tried again within 180 s'};"
                f" writable again, {'a line written' if written else 'no line within 180 s'}")
        ok = failed_event and second and 45 <= again <= 120 and written
        return ("PASS" if ok else "FAIL"), said
    finally:
        # Each step whatever the one before did (review); the switch first,
        # and never back to on.
        steps = []
        if switched:
            steps.append(lambda: set_permission_live(ctx, "away_lock", False if had is False else None))
        if before is not None:
            steps.append(lambda: ctx.server.restore_config(before))
        steps.append(lambda: lock.exists() and os.chmod(lock, stat.S_IREAD | stat.S_IWRITE))
        steps.append(lambda: lock.unlink(missing_ok=True))
        for step in steps:
            try:
                step()
            except (OSError, ValueError) as error:
                print(f"note: an AG1 cleanup step failed: {error}", file=sys.stderr)
# ---- the theme follows the system (AG3) ----


def parse_raw(data: bytes) -> Optional[tuple[int, int, bytes]]:
    """screencap's raw form as `(width, height, 4-byte pixels)`: a 12-byte
    header (width, height, format), or 16 with a colour space after it."""
    import struct
    if len(data) < 16:
        return None
    width, height, _ = struct.unpack("<III", data[:12])
    for header in (16, 12):
        if len(data) - header == width * height * 4:
            return width, height, data[header:]
    return None


def raw_shot() -> Optional[tuple[int, int, bytes]]:
    """The phone's screen, from `adb exec-out screencap`."""
    return parse_raw(subprocess.run(["adb", "exec-out", "screencap"], capture_output=True, timeout=30).stdout)


def brightness(shot: tuple[int, int, bytes], x: int, y: int, size: int = 30) -> float:
    """The mean of R, G and B over a `size` square at (x, y)."""
    width, height, pixels = shot
    total = count = 0
    for row in range(y, min(y + size, height)):
        for col in range(x, min(x + size, width)):
            at = (row * width + col) * 4
            total += pixels[at] + pixels[at + 1] + pixels[at + 2]
            count += 3
    return total / count if count else 0.0


def night_mode() -> str:
    out = adb("shell", "cmd", "uimode", "night")
    found = re.search(r"Night mode: (\w+)", out)
    return found.group(1) if found else ""


@check("ag3-theme", "§21")
def ag3_theme(ctx: Ctx) -> tuple[str, str]:
    """With the app's theme following the system (its default), the system's
    dark mode turned on and off restyles Home at once, without the app
    starting again (§21): the app's process keeps running (the activity is
    recreated, by design). Read from screenshots: a stretch of Home's
    background at its very left edge. The system's mode is put back after."""
    if ctx.phone.prefs("pocketdesk-ui").get("theme_mode", "system") != "system":
        return "SKIP", "the app's theme is set in its Settings, not the system's"
    was = night_mode()
    if was not in ("yes", "no"):
        return "SKIP", f"the system's night mode is {was or 'unread'}: it could not be put back as it was"
    try:
        ctx.phone.wake()
        adb("shell", "monkey", "-p", APP, "-c", "android.intent.category.LAUNCHER", "1")
        if not wait_for(lambda: shows(ctx, "Your saved computers"), 15, 1):
            return "SKIP", "the app's Home did not show"
        pid = adb("shell", "pidof", APP).strip()
        seen = {}
        for mode in ("no", "yes", "no"):
            adb("shell", "cmd", "uimode", "night", mode)
            time.sleep(3)
            shot = raw_shot()
            if shot is None:
                return "TIMEOUT", "no screenshot"
            # Home's background: its very left edge, halfway down, inside the
            # cards' 20 dp margin on any density (review).
            seen.setdefault(mode, []).append(brightness(shot, 2, shot[1] // 2, size=12))
        same = adb("shell", "pidof", APP).strip() == pid
        light, dark = min(seen["no"]), max(seen["yes"])
        said = (f"light {', '.join(f'{b:.0f}' for b in seen['no'])}, dark {seen['yes'][0]:.0f} (0-255);"
                f" the app's process {'kept running' if same else 'started again'}")
        return ("PASS" if light > 170 and dark < 90 and same else "FAIL"), said
    finally:
        adb("shell", "cmd", "uimode", "night", was)


# ---- a phone the server does not know, by the internet road (AL2) ----


@check("al2-unpaired-road", "§1")
def al2_unpaired_road(ctx: Ctx) -> tuple[str, str]:
    """A phone the server has not paired, dialing by the internet road, is
    refused (§1, the QUIC gate over WAN): the server's log says so, nothing
    links, and no session sends a frame. It is refused at whichever door it
    reaches: its control link (the road keeps nothing for pairing), or, if
    it dials a session straight, the session's gate, whose record (kept for
    that road alone) then ends `rejected`. The phone's own "session
    connected" is no sign: it is logged once its link is up and its wishes
    sent, before the computer's gate has read them.
    The phone is taken out of the throwaway's paired list for it, its keys
    left as they are so the pairing alone refuses, and the list is put back
    after. Without the reverse, the phone's 127.0.0.1 is the phone itself:
    whatever reaches the server came by the road. Pairing is required in
    the config for it, as it is by default: without it an unpaired phone
    gets the default permissions, input among them."""
    path = ctx.server.root / "state" / "trusted_devices.json"
    kept = path.read_bytes()
    store = json.loads(kept)
    if ctx.device not in store.get("trusted_device_ids", []):
        return "SKIP", "the phone is not paired with the throwaway"
    changed = False
    before = None
    try:
        ctx.server.stop()
        before = ctx.server.configure({"pairing_required": "true"})
        adb("reverse", "--remove", f"tcp:{ctx.phone_port}")
        store["trusted_device_ids"] = [d for d in store["trusted_device_ids"] if d != ctx.device]
        tmp = path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(store, indent=2), encoding="utf-8")
        os.replace(tmp, path)
        changed = True
        ctx.on_road = False
        why = ctx.server.start()
        if why:
            return "TIMEOUT", why
        records = len(session_records(ctx.server.root))
        plain = lambda: re.sub(chr(27) + r"\[[0-9;]*m", "", ctx.server.log())
        control = f"Refused {ctx.device}'s control connection over the internet"
        session = "Streaming session rejected: device not paired"
        # A road dial that failed while the server restarted rests 60 s
        # before the phone's next try (as in by_road): a second Connect.
        for _ in range(2):
            if start_session(ctx) is None:
                return "SKIP", "no Connect on the throwaway's card"
            if wait_for(lambda: control in plain() or session in plain(), 120, 2):
                break
        else:
            # The phone's own words of it, kept beside the throwaway.
            (ctx.server.root / "phone_al2.txt").write_text(
                adb("logcat", "-d", "-s", "PocketdeskRust:*"), encoding="utf-8")
            return "TIMEOUT", "nothing refused after two Connects, 120 s each (the phone's log: phone_al2.txt)"
        # The rest of the phone's tries, refused or not.
        time.sleep(20)
        log = plain()
        doors = [door for door, line in (("its control link", control), ("its session", session)) if line in log]
        rejected = [r for r in session_records(ctx.server.root)[records:] if r.get("outcome") == "rejected"]
        framed = [r for r in session_records(ctx.server.root)[records:] if r.get("enc_n", "0") != "0"]
        linked = bool(iroh_loops(log)) or ctx.device in ctx.server.linked()
        said = (f"refused at {' and '.join(doors)} in the server's log; {len(rejected)} session records"
                f" rejected, {len(framed)} with frames; the phone {'linked' if linked else 'never linked'}")
        ok = not framed and not linked and (session not in log or rejected)
        return ("PASS" if ok else "FAIL"), said
    finally:
        # Each step whatever the one before did; the paired list back before
        # the server starts again, and the phone home.
        def home() -> None:
            why = by_home(ctx)
            if why:
                print(f"note: AL2 left the phone off its home link: {why}", file=sys.stderr)

        def paired_again() -> None:
            tmp = path.with_suffix(".json.tmp")
            tmp.write_bytes(kept)
            os.replace(tmp, path)

        for step in (lambda: adb("shell", "input", "keyevent", "KEYCODE_BACK"), ctx.server.stop):
            try:
                step()
            except (OSError, ValueError) as error:
                print(f"note: an AL2 cleanup step failed: {error}", file=sys.stderr)
        try:
            if changed:
                paired_again()
        except OSError as error:
            # The phone still unpaired: the config keeps pairing required and
            # nothing starts the server again (review).
            print(f"note: AL2 could not put the paired list back ({error}); the server is left stopped,"
                  f" pairing required: put {path} back by hand", file=sys.stderr)
        else:
            for step in ([lambda: ctx.server.restore_config(before)] if before is not None else []) + [home]:
                try:
                    step()
                except (OSError, ValueError) as error:
                    print(f"note: an AL2 cleanup step failed: {error}", file=sys.stderr)


# ---- the clipboard history, empty (AL3) ----


def node_box(dump: str, starts: str) -> Optional[tuple[int, int, int, int]]:
    """The bounds of the first view whose text starts with `starts`."""
    for node in re.findall(r"<node [^>]*>", dump):
        text = re.search(r' text="([^"]*)"', node)
        box = re.search(r'bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', node)
        if text and box and html.unescape(text.group(1)).startswith(starts):
            return tuple(int(v) for v in box.groups())  # type: ignore[return-value]
    return None


def marked_share(shot: tuple[int, int, bytes], box: tuple[int, int, int, int],
                 ground: tuple[int, int]) -> float:
    """How much of `box` (left, top, right, bottom) differs from the pixel
    at `ground` by more than 40 in a colour: something drawn there."""
    width, height, pixels = shot
    at = (ground[1] * width + ground[0]) * 4
    base = pixels[at:at + 3]
    left, top, right, bottom = box
    total = drawn = 0
    for row in range(max(top, 0), min(bottom, height)):
        for col in range(max(left, 0), min(right, width)):
            at = (row * width + col) * 4
            total += 1
            if any(abs(pixels[at + i] - base[i]) > 40 for i in range(3)):
                drawn += 1
    return drawn / total if total else 0.0


@check("al3-empty-history", "§21")
def al3_empty_history(ctx: Ctx) -> tuple[str, str]:
    """The clipboard history sheet with nothing kept: its words, its
    clipboard icon above them, and no Clear (§21, §125). The test phone's
    entries are cleared for it, as its Clear does, confirmed; they are
    counted, never read. The icon has no words of its own (it is
    decoration), so it is read from a screenshot kept in memory: a 40 dp
    square 8 dp above the words with something drawn in it."""
    ctx.phone.wake()
    adb("shell", "monkey", "-p", APP, "-c", "android.intent.category.LAUNCHER", "1")
    if not wait_for(lambda: tap_text("More for Test Desk", ctx.phone.dump()), 20, 2):
        return "SKIP", "no Test Desk card on Home"
    time.sleep(1)
    if not tap_text("Send from clipboard history", ctx.phone.dump()):
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        return "FAIL", "no Send from clipboard history in the card's menu"
    time.sleep(1.5)
    try:
        sheet = ctx.phone.dump()
        cleared = len(history_rows(sheet))
        if cleared:
            if not tap_text("Clear", sheet):
                return "FAIL", f"{cleared} rows and no Clear"
            time.sleep(1)
            dialog = ctx.phone.dump()
            if "This deletes every saved clipboard entry from this phone." not in dialog:
                return "FAIL", "Clear did not ask first"
            # The dialog's Clear: the button under its words (its title is
            # above them), whatever order the dump lists views in (review).
            words_at = next(y for text, _, y in dump_nodes(dialog)
                            if text.startswith("This deletes every saved clipboard entry"))
            spots = [(x, y) for text, x, y in dump_nodes(dialog) if text == "Clear" and y > words_at]
            if not spots:
                return "FAIL", "no Clear under the dialog's words"
            adb("shell", "input", "tap", str(spots[0][0]), str(spots[0][1]))
            time.sleep(1.5)
            sheet = ctx.phone.dump()
        words = node_box(sheet, "Nothing copied yet")
        has_clear = any(text == "Clear" for text, _, _ in dump_nodes(sheet))
        if not words:
            return "FAIL", f"{len(history_rows(sheet))} rows left and no empty words after {cleared} cleared"
        shot = raw_shot()
        if not shot:
            return "FAIL", "no screenshot"
        dp = density()
        middle = (words[0] + words[2]) // 2
        icon = (middle - int(20 * dp), words[1] - int(48 * dp), middle + int(20 * dp), words[1] - int(8 * dp))
        # The sheet's own colour beside the icon, at its height.
        ground = (middle + int(60 * dp), words[1] - int(28 * dp))
        share = marked_share(shot, icon, ground)
        said = (f"{cleared} entries cleared; the empty words, {'a' if has_clear else 'no'} Clear,"
                f" {share:.0%} of the icon's square drawn")
        return ("PASS" if not has_clear and share >= 0.03 else "FAIL"), said
    finally:
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        time.sleep(1)
        if any(text == "Clipboard history" for text, _, _ in dump_nodes(ctx.phone.dump())):
            adb("shell", "input", "keyevent", "KEYCODE_BACK")


# ---- the phone's ringer on the computer (AP1) ----


def ringer_of(root: pathlib.Path, device: str) -> Optional[str]:
    """`device`'s ringer as the computer keeps it (`phone_status.json`)."""
    try:
        data = json.loads((root / "state" / "phone_status.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return data.get("batteries", {}).get(device, {}).get("ringer")


def ringer_mode(dump: str) -> Optional[str]:
    """The phone's ringer, from `dumpsys audio`: normal, vibrate or silent."""
    found = re.search(r"mode \(external\)\s*=\s*(\w+)", dump) or re.search(r"mRingerMode=(\w+)", dump)
    return found.group(1).lower() if found else None


def phone_ringer() -> Optional[str]:
    return ringer_mode(adb("shell", "dumpsys", "audio"))


# `settings get global zen_mode` to `cmd notification set_dnd`'s words.
DND_WORDS = {0: "off", 1: "priority", 2: "none", 3: "alarms"}


def zen_mode() -> Optional[int]:
    """The phone's Do Not Disturb (0 off, 1 priority, 2 total silence, 3
    alarms only)."""
    said = adb("shell", "settings", "get", "global", "zen_mode").strip()
    return int(said) if said.isdigit() else None


def set_ringer(mode: str) -> None:
    """Through the fixture (the shell cannot on Android 14)."""
    am("broadcast", "-n", f"{FIXTURE}/.RingerReceiver", "-a", "dev.pocketdesk.fixture.RINGER", "--es", "mode", mode)


@check("ap1-ringer", "§162")
def ap1_ringer(ctx: Ctx) -> tuple[str, str]:
    """The phone's ringer reaches the computer: turned on the phone in the
    background, the computer's copy follows within a tick (the phone sends
    its state when the ringer changes); in a session, within 15 s (§162).
    The phone's ringer is put back as it was, and its Do Not Disturb: silent
    turns it on (AR3: left on, the phone held back every notification the
    later checks sent)."""
    if not ctx.phone.prefs("pocketdesk-ui").get("share_battery", True):
        return "SKIP", "turn on Share the phone's state in the phone's Settings"
    was = phone_ringer()
    if was not in ("normal", "vibrate", "silent"):
        return "SKIP", f"the phone's ringer reads {was or 'nothing'}: it could not be put back"
    dnd = zen_mode()
    if dnd not in DND_WORDS:
        return "SKIP", f"the phone's Do Not Disturb reads {dnd}: it could not be put back"
    first, second = ("vibrate", "silent") if was != "vibrate" else ("silent", "normal")
    said = []
    # Silent, and leaving it, needs Do Not Disturb access: the fixture's
    # for the check, taken back after.
    adb("shell", "cmd", "notification", "allow_dnd", FIXTURE)
    try:
        start = time.monotonic()
        set_ringer(first)
        seen = wait_for(lambda: ringer_of(ctx.server.root, ctx.device) == first, 90, 1)
        said.append(f"{first} in the background: {f'{time.monotonic() - start:.0f} s' if seen else 'not within 90 s'}")
        if not seen:
            return "FAIL", "; ".join(said)

        def body(ctx: Ctx) -> tuple[str, str]:
            if start_session(ctx) is None:
                return "SKIP", "no Connect on a 127.0.0.1 card on Home"
            if not wait_for(lambda: in_session(ctx), 30, 1):
                return "TIMEOUT", "no session within 30 s"
            at = time.monotonic()
            set_ringer(second)
            # Watched past the 15 s it must keep, so a miss says by how much.
            got = wait_for(lambda: ringer_of(ctx.server.root, ctx.device) == second, 30, 0.5)
            took = time.monotonic() - at
            said.append(f"{second} in a session: {f'{took:.1f} s' if got else 'not within 30 s'}")
            return ("PASS" if got and took <= 15 else "FAIL"), "; ".join(said)

        return session_check(ctx, body)
    finally:
        set_ringer(was)
        time.sleep(1)
        # The ringer's silent and back turn Do Not Disturb on and off as they go.
        # Where both cannot be put back (on the OPPO silent is Do Not Disturb:
        # silent with it off cannot be set), Do Not Disturb wins, as the later
        # checks need the phone's notifications; the ringer's note says so.
        if zen_mode() != dnd:
            adb("shell", "cmd", "notification", "set_dnd", DND_WORDS[dnd])
            time.sleep(1)
        adb("shell", "cmd", "notification", "disallow_dnd", FIXTURE)
        if phone_ringer() != was:
            print(f"note: AP1 left the phone's ringer at {phone_ringer()}, not {was}", file=sys.stderr)
        if zen_mode() != dnd:
            print(f"note: AP1 left the phone's Do Not Disturb at {zen_mode()}, not {dnd}", file=sys.stderr)


# ---- an unpaired phone's reports leave the computer (AP2) ----


def kept_for(root: pathlib.Path, device: str) -> list[str]:
    """Which of the computer's per-phone reports still hold `device`."""
    out = []
    for name, key in (("phone_switches.json", "phones"), ("phone_status.json", "batteries")):
        try:
            data = json.loads((root / "state" / name).read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if device in data.get(key, {}):
            out.append(name)
    return out


@check("ap2-unpaired-reports", "§163")
def ap2_unpaired_reports(ctx: Ctx) -> tuple[str, str]:
    """A phone unpaired (`pocketdesk unpair`, as the desktop app's Unpair
    does) is gone within seconds from the computer's reports of it: its
    switches (`phone_switches.json`) and its state (`phone_status.json`)
    (§163). The throwaway's trust store and device keys are put back after,
    and the phone linked at home again."""
    state = ctx.server.root / "state"
    kept = {name: (state / name).read_bytes() for name in ("trusted_devices.json", "device_keys.json")
            if (state / name).exists()}
    # A server just started has the phone's reports once it has sent them.
    wait_for(lambda: len(kept_for(ctx.server.root, ctx.device)) == 2, 90, 1)
    before = kept_for(ctx.server.root, ctx.device)
    if "phone_switches.json" not in before:
        return "SKIP", "the computer kept no switches report for the phone within 90 s"
    unpaired = False
    try:
        # Set before the CLI: one that timed out may still have unpaired (review).
        unpaired = True
        code, out = ctx.server.cli("unpair", ctx.device)
        if code != 0:
            return "FAIL", f"pocketdesk unpair said {code}: {out.strip()[:120]}"
        start = time.monotonic()
        gone = wait_for(lambda: not kept_for(ctx.server.root, ctx.device), 30, 0.5)
        took = time.monotonic() - start
        left = kept_for(ctx.server.root, ctx.device)
        said = (f"held in {', '.join(before)}; unpaired: gone from them in {took:.0f} s" if gone
                else f"held in {', '.join(before)}; unpaired: still in {', '.join(left)} after 30 s")
        return ("PASS" if gone and took <= 15 else "FAIL"), said
    finally:
        if unpaired:
            # The pairing back as it was: stopped, the files put back, started;
            # each step whatever the one before did, as AL2's (review).
            try:
                ctx.server.stop()
            except (OSError, ValueError) as error:
                print(f"note: an AP2 cleanup step failed: {error}", file=sys.stderr)
            missed = []
            for name, data in kept.items():
                # A server killed at its stop may hold the file a moment.
                for attempt in range(5):
                    try:
                        tmp = state / f"{name}.tmp"
                        tmp.write_bytes(data)
                        os.replace(tmp, state / name)
                        break
                    except OSError as error:
                        if attempt == 4:
                            missed.append(f"{name} ({error})")
                        time.sleep(1)
            if missed:
                print(f"note: AP2 could not put back {', '.join(missed)}; the server is left stopped,"
                      f" the phone unpaired: put them back in {state} by hand", file=sys.stderr)
            else:
                try:
                    # Unpaired, the phone may have gone to the internet road
                    # and gone back to it: once more, then a note.
                    why = by_home(ctx)
                    if why and "internet road" in why:
                        why = by_home(ctx)
                    if why:
                        print(f"note: AP2 left the phone off its home link: {why}", file=sys.stderr)
                except (OSError, ValueError) as error:
                    print(f"note: an AP2 cleanup step failed: {error}", file=sys.stderr)


# ---- the siren in a session (AQ1) ----

SIREN_TITLE = "Your computer is finding this phone"


def alarm_playing(dump: str) -> bool:
    """Whether something plays on the alarm stream now, by `dumpsys audio`'s
    players. Not the app's own process: Android plays its ringtone from
    SystemUI's (seen on the OPPO), so a check makes sure none played before."""
    return any("AudioPlaybackConfiguration" in line and "state:started" in line
               and "usage=USAGE_ALARM" in line for line in dump.splitlines())


def siren_up(ctx: Ctx) -> bool:
    return node_box(ctx.phone.dump(), SIREN_TITLE) is not None


def alarm_now() -> bool:
    return alarm_playing(adb("shell", "dumpsys", "audio"))


def alarm_volume() -> Optional[int]:
    return volume_of(adb("shell", "cmd", "media_session", "volume", "--stream", "4", "--get"))


@check("aq1-siren", "§21")
def aq1_siren(ctx: Ctx) -> tuple[str, str]:
    """In a session, a ring from the computer shows the siren's dialog
    while the alarm plays; Silence stops both at once, and a ring left
    alone stops both after 30 s (§21). The second ring comes a minute
    after the first, as the computer rings a phone at most once a minute.
    The ring raises the alarm's volume, and the app must put it back (§202)."""
    def ring() -> tuple[Optional[str], float]:
        """Why the ring did not show, if it did not; when it was asked."""
        if alarm_now():
            return "an alarm was playing before the ring", time.monotonic()
        code, out = ctx.server.cli("ring", ctx.device)
        asked = time.monotonic()
        if code != 0:
            return (out.strip().splitlines()[-1] if out.strip() else f"pocketdesk ring exit {code}"), asked
        if not wait_for(lambda: siren_up(ctx), 15, 1):
            return "no siren dialog within 15 s of the ring", asked
        if not wait_for(alarm_now, 5, 0.5):
            return "the dialog up, but no alarm playing", asked
        return None, asked

    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        volume = alarm_volume()
        said = []
        try:
            why, first = ring()
            if why:
                return "FAIL", why
            # The phone's own dialog: a tap on the phone, never toward the computer.
            if not tap_text("Silence", ctx.phone.dump()):
                return "FAIL", "the dialog had no Silence"
            at = time.monotonic()
            stopped = wait_for(lambda: not alarm_now() and not siren_up(ctx), 10, 0.5)
            said.append(f"Silence: {f'both stopped in {time.monotonic() - at:.1f} s' if stopped else 'not stopped within 10 s'}")
            if not stopped:
                return "FAIL", "; ".join(said)
            if not in_session(ctx):
                return "FAIL", "; ".join(said + ["the session ended"])
            # The computer rings a phone at most once a minute, counted from
            # when it took the ask: a margin past it (review).
            time.sleep(max(0.0, first + 62 - time.monotonic()))
            why, asked = ring()
            if why:
                return "FAIL", "; ".join(said + [f"the second ring: {why}"])
            stopped = wait_for(lambda: not alarm_now() and not siren_up(ctx), 45, 1)
            # From the ask (review): the ring's 30 s start a moment after it.
            took = time.monotonic() - asked
            said.append(f"left alone: {f'both stopped {took:.0f} s after the ring' if stopped else 'not stopped within 45 s'}")
            ok = stopped and 27 <= took <= 40 and in_session(ctx)
            # The volume is put back once the ring has stopped.
            back = wait_for(lambda: alarm_volume() == volume, 10, 1) if volume is not None else False
            said.append(f"the alarm's volume {'back at' if back else 'not back at'} {volume}")
            return ("PASS" if ok and back else "FAIL"), "; ".join(said)
        finally:
            # A ring left going by a check that stopped part way is silenced,
            # or waited out, so the next check starts quiet (review).
            if siren_up(ctx):
                tap_text("Silence", ctx.phone.dump())
            if not wait_for(lambda: not alarm_now(), 40, 2):
                print("note: AQ1 left an alarm playing on the phone", file=sys.stderr)

    return session_check(ctx, body)


# ---- Home with a second computer (AQ2) ----

# A documentation address (RFC 5737): nothing answers there.
SECOND_HOST = "192.0.2.10"


def host_addresses(raw: Optional[str]) -> list[str]:
    """The saved computers' addresses, in Home's order (HostStore's list)."""
    try:
        return [str(h.get("address", "")) for h in json.loads(raw or "[]")]
    except (ValueError, AttributeError):
        return []


def saved_hosts(ctx: Ctx) -> Optional[str]:
    return ctx.phone.prefs("pocketdesk-host").get("hosts")  # type: ignore[return-value]


HOME_MARK = "Your saved computers"


def home(ctx: Ctx) -> bool:
    """Home, scrolled to its top (it keeps where it was); False when the
    app came up on anything else, which is then not swiped (a session's
    surface would take it as touch toward a computer)."""
    ctx.phone.wake()
    adb("shell", "monkey", "-p", APP, "-c", "android.intent.category.LAUNCHER", "1")
    if not wait_for(lambda: shows(ctx, HOME_MARK), 10, 1):
        # Home's form, left open (AV1): its Back is Home, the first perhaps
        # only putting the keyboard away; Back only while the form shows.
        for _ in range(2):
            texts = {t for t, _, _ in dump_nodes(ctx.phone.dump())}
            if not texts & {"Computer's address", "Advanced (ports, wake-on-LAN)", "Hide advanced"}:
                break
            adb("shell", "input", "keyevent", "KEYCODE_BACK")
            time.sleep(1.5)
        if not wait_for(lambda: shows(ctx, HOME_MARK), 10, 1):
            return False
    for _ in range(3):
        adb("shell", "input", "swipe", "540", "700", "540", "1900", "200")
    time.sleep(1)
    return True


def card_menu(dump: str, address: str) -> Optional[tuple[int, int, str]]:
    """The menu button of the card that shows `address`, as its title or,
    when it has a name, at the start of its subtitle: the "More for …"
    beside that line, with the name it gives the card."""
    sub = next((y for text, _, y in dump_nodes(dump) if text.startswith(address)), None)
    if sub is None:
        return None
    menus = [((int(a) + int(c)) // 2, (int(b) + int(d)) // 2, html.unescape(label))
             for label, a, b, c, d in re.findall(
                 r'<node [^>]*content-desc="More for ([^"]*)"[^>]*bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', dump)]
    near = [m for m in menus if abs(sub - m[1]) <= 150]
    return near[0] if near else None


def phone_records(ctx: Ctx) -> dict[str, object]:
    """The phone's saved computers and every record kept with them, but
    SECOND_HOST's; auto-connect as a yes or no (the form writes it)."""
    kept = {k: v for k, v in ctx.phone.prefs("pocketdesk-host").items() if not k.endswith(f":{SECOND_HOST}")}
    kept["auto_connect"] = bool(kept.get("auto_connect"))
    return kept


def add_second_host(ctx: Ctx, mac: Optional[str] = None,
                    on_sheet: Optional[Callable[[str], bool]] = None,
                    address: str = SECOND_HOST) -> Optional[str]:
    """Add `address` through Home's form, with a wake-on-LAN `mac` when
    given (AV1); why not, when it was not. Its Connect saves it and dials:
    the pairing sheet that comes is cancelled, unless `on_sheet`, given
    the sheet, says it saw it through (AV2); the session that tries or
    opens is left with one Back, to Home."""
    if not home(ctx):
        return "the app did not open on Home"
    if not tap_scrolling(ctx, "Add computer", swipes=4):
        return "no Add computer on Home"
    box = None
    if wait_for(lambda: shows(ctx, "Computer's address"), 10, 1):
        box = next((tuple(map(int, b)) for b in re.findall(
            r'<node [^>]*class="android.widget.EditText"[^>]*bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"',
            ctx.phone.dump())), None)
    if box is None:
        return "the form had no address field"
    adb("shell", "input", "tap", str((box[0] + box[2]) // 2), str((box[1] + box[3]) // 2))
    time.sleep(0.5)
    adb("shell", "input", "text", address)
    time.sleep(0.5)
    if mac:
        if not tap_text("Advanced (ports, wake-on-LAN)", ctx.phone.dump()):
            home(ctx)
            return "the form had no Advanced"
        if not wait_for(lambda: tap_text("Wake-on-LAN MAC address", ctx.phone.dump()), 10, 1):
            home(ctx)
            return "the form had no wake-on-LAN field"
        time.sleep(0.5)
        adb("shell", "input", "text", mac)
        time.sleep(0.5)
        # The keyboard covers Connect: Back while it shows only puts it
        # away (never the wake-on-LAN field's Send).
        if keyboard_shown():
            adb("shell", "input", "keyevent", "KEYCODE_BACK")
            time.sleep(1)
    if not tap_scrolling(ctx, "Connect", swipes=3):
        home(ctx)
        return "the form had no Connect"
    sheet = ""

    def sheet_up() -> bool:
        nonlocal sheet
        sheet = ctx.phone.dump()
        return any(t == "Cancel" for t, _, _ in dump_nodes(sheet)) and node_box(sheet, f"Pair with {address}") is not None

    if not wait_for(sheet_up, 20, 1):
        return f"no pairing sheet for {address} within 20 s"
    if not (on_sheet and on_sheet(sheet)):
        tap_text("Cancel", ctx.phone.dump())
    time.sleep(2)
    adb("shell", "input", "keyevent", "KEYCODE_BACK")
    # Once: a second Back on Home would close the app (review).
    if not wait_for(lambda: shows(ctx, HOME_MARK), 10, 1):
        return "Back did not come to Home"
    return None


def remove_second_host(ctx: Ctx, cleanup: bool = False, address: str = SECOND_HOST) -> Optional[str]:
    """Remove `address`'s card through its menu, confirmed only when the
    dialog names that card. A card with a name (as before AQ3, or one that
    paired) is removed only as cleanup, found by the address on its
    subtitle."""
    if not home(ctx):
        return "the app did not open on Home"
    spot = None
    for _ in range(4):
        spot = card_menu(ctx.phone.dump(), address)
        if spot:
            break
        adb("shell", "input", "swipe", "540", "1900", "540", "1100", "300")
        time.sleep(1)
    if spot is None:
        return "no menu on its card"
    x, y, label = spot
    if label != address and not cleanup:
        return f"its card is named {label!r}, not {address}"
    adb("shell", "input", "tap", str(x), str(y))
    time.sleep(1)
    if not tap_scrolling(ctx, "Remove", swipes=3):
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        return "no Remove in its menu"
    asks = f"Remove {label} from this phone?"
    if not wait_for(lambda: node_box(ctx.phone.dump(), asks) is not None, 10, 1):
        # Another card's, or no question: nothing removed (review).
        tap_text("Cancel", ctx.phone.dump())
        return f"the dialog did not ask to remove {label}"
    buttons = [(bx, by) for text, bx, by in dump_nodes(ctx.phone.dump()) if text == "Remove"]
    if len(buttons) < 2:
        tap_text("Cancel", ctx.phone.dump())
        return "the dialog had no Remove button"
    # The button, after the dialog's title of the same word.
    adb("shell", "input", "tap", str(buttons[-1][0]), str(buttons[-1][1]))
    time.sleep(2)
    return None


def keyboard_shown() -> bool:
    """Whether the phone's keyboard is up (`dumpsys input_method`)."""
    return "mInputShown=true" in adb("shell", "dumpsys", "input_method")


def card_menu_items(ctx: Ctx, address: str) -> Optional[list[str]]:
    """The entries of the menu of the card that shows `address`, read and
    closed again (Back); None when there is no such card."""
    if not home(ctx):
        return None
    spot = None
    for _ in range(4):
        spot = card_menu(ctx.phone.dump(), address)
        if spot:
            break
        adb("shell", "input", "swipe", "540", "1900", "540", "1100", "300")
        time.sleep(1)
    if spot is None:
        return None
    adb("shell", "input", "tap", str(spot[0]), str(spot[1]))
    time.sleep(1.5)
    items = [t for t, _, _ in dump_nodes(ctx.phone.dump()) if t]
    adb("shell", "input", "keyevent", "KEYCODE_BACK")
    time.sleep(1)
    return items


def pin_sheet_facts(dump: str) -> tuple[Optional[bool], Optional[bool]]:
    """The pairing sheet's PIN field: whether it has focus, and whether the
    PIN shows (its eye offers "Hide PIN") or is masked ("Show PIN")."""
    field = next((n for n in re.findall(r"<node [^>]*>", dump) if 'class="android.widget.EditText"' in n), None)
    focused = None if field is None else 'focused="true"' in field
    shows_pin = True if 'content-desc="Hide PIN"' in dump else False if 'content-desc="Show PIN"' in dump else None
    return focused, shows_pin


# A documentation MAC (RFC 7042): no machine has it.
DOC_MAC = "00:00:5E:00:53:01"


@check("av1-wake-row", "§20")
def av1_wake_row(ctx: Ctx) -> tuple[str, str]:
    """A card's menu offers Wake only for a computer the phone knows how to
    wake (§20, N6): one added at a documentation address with no MAC has
    none; added again with a documentation MAC, it has Wake; the
    throwaway's, which announced how to wake it, has Wake. Nothing is
    woken. The phone's saved computers and their records as they were
    after."""
    before = saved_hosts(ctx)
    records = phone_records(ctx)
    if SECOND_HOST in host_addresses(before):
        return "SKIP", f"{SECOND_HOST} is already saved on the phone"
    said = []
    try:
        own = card_menu_items(ctx, "127.0.0.1")
        if own is None:
            return "SKIP", "no 127.0.0.1 card on Home"
        if "Wake" not in own:
            return "FAIL", "the throwaway's card, which announced how to wake it, offers no Wake"
        said.append("the throwaway's: Wake")
        for mac, step in ((None, "no MAC"), (DOC_MAC, "a MAC")):
            why = add_second_host(ctx, mac)
            if why:
                return "FAIL", "; ".join(said + [f"{step}: {why}"])
            items = card_menu_items(ctx, SECOND_HOST)
            if items is None:
                return "FAIL", "; ".join(said + [f"{step}: no menu on its card"])
            if "Wake settings" not in items:
                return "FAIL", "; ".join(said + [f"{step}: no Wake settings in its menu"])
            if ("Wake" in items) != (mac is not None):
                return "FAIL", "; ".join(said + [f"{step}: Wake {'offered' if 'Wake' in items else 'not offered'}"])
            said.append(f"{step}: {'Wake' if mac else 'no Wake'}")
            why = remove_second_host(ctx)
            if why or saved_hosts(ctx) != before:
                return "FAIL", "; ".join(said + [f"{step}: {why or 'the saved computers not as they were'}"])
        after = phone_records(ctx)
        changed = sorted({k.split(":")[0] for k in set(records) | set(after) if records.get(k) != after.get(k)})
        if changed:
            return "FAIL", "; ".join(said + [f"records changed: {', '.join(changed)}"])
        return "PASS", "; ".join(said)
    finally:
        if SECOND_HOST in host_addresses(saved_hosts(ctx)):
            why = remove_second_host(ctx, cleanup=True)
            if why or SECOND_HOST in host_addresses(saved_hosts(ctx)):
                print(f"note: AV1 left {SECOND_HOST} saved on the phone: {why or 'still there'}", file=sys.stderr)


# The throwaway by another name: the phone keeps no pairing for it, so its
# Connect brings the pairing sheet (AV2).
OTHER_NAME = "localhost"


@check("av2-pin-sheet", "§21")
def av2_pin_sheet(ctx: Ctx) -> tuple[str, str]:
    """The pairing sheet with a computer that answers (§21): a card for the
    throwaway by another name (localhost), with a PIN put up by `pocketdesk
    pair`; Show a PIN on the computer says one is showing, the PIN field
    has focus, and the PIN is masked or shown as "reveal the PIN" says;
    the PIN typed, the sheet says paired and goes by itself. The card is
    removed, and the throwaway's trust store and device keys put back."""
    config = ctx.server.root / "config" / "config.toml"
    if re.search(r"(?m)^\s*pairing_required\s*=\s*false", config.read_text(encoding="utf-8") if config.exists() else ""):
        return "SKIP", "the throwaway's config turns pairing_required off"
    if OTHER_NAME in host_addresses(saved_hosts(ctx)):
        return "SKIP", f"{OTHER_NAME} is already saved on the phone"
    # The form's control port is its default, 1716 (review).
    if ctx.phone_port != 1716:
        return "SKIP", f"the phone reaches the throwaway on {ctx.phone_port}, not the form's 1716"
    # A phone the throwaway does not trust would be paired with the
    # defaults, input too, by the owner's own PIN (review).
    if ctx.device not in trusted_store(ctx).get("trusted_device_ids", []):
        return "SKIP", "the phone is not paired with the throwaway"
    state = ctx.server.root / "state"
    kept = {name: (state / name).read_bytes() for name in ("trusted_devices.json", "device_keys.json")
            if (state / name).exists()}
    before = saved_hosts(ctx)
    records = phone_records(ctx)
    reveal = bool(ctx.phone.prefs("pocketdesk-ui").get("reveal_pin"))
    seen: dict[str, object] = {}
    code, out = ctx.server.cli("pair", "--json")
    line = next((l for l in reversed(out.splitlines()) if l.strip().startswith("{")), "")
    try:
        pin = str(json.loads(line)["pin"])
    except (ValueError, KeyError, TypeError):
        return "FAIL", f"pocketdesk pair said {code}, no PIN"

    def through(sheet: str) -> bool:
        if not tap_text("Show a PIN on the computer", sheet):
            return False
        if not wait_for(lambda: shows(ctx, "A PIN is showing on the computer. Type it below."), 15, 1):
            seen["ready"] = False
            return False
        seen["ready"] = True
        time.sleep(1)
        seen["focused"], seen["shown"] = pin_sheet_facts(ctx.phone.dump())
        # Typed where the focus is, as a thumb would.
        adb("shell", "input", "text", pin)
        time.sleep(0.5)
        tap_text("Pair", ctx.phone.dump())
        # "Paired ✓" shows 0.9 s, under a screen read's time: the phone's own
        # record of the pairing is the proof, then the sheet gone.
        seen["paired"] = bool(wait_for(
            lambda: ctx.phone.prefs("pocketdesk-host").get(f"paired_desktop:{OTHER_NAME}"), 15, 1))
        seen["gone"] = bool(wait_for(lambda: not node_box(ctx.phone.dump(), f"Pair with {OTHER_NAME}"), 10, 1))
        return True

    try:
        why = add_second_host(ctx, on_sheet=through, address=OTHER_NAME)
        if why:
            return "FAIL", f"{why}; {seen}"
        if not seen.get("ready"):
            return "FAIL", "Show a PIN on the computer did not say one is showing"
        wrong = []
        if seen.get("focused") is not True:
            wrong.append("the PIN field without focus")
        if seen.get("shown") is not reveal:
            wrong.append(f"the PIN {'shown' if seen.get('shown') else 'masked'} with reveal_pin {reveal}")
        if not seen.get("paired"):
            wrong.append("not paired after the PIN")
        if not seen.get("gone"):
            wrong.append("the sheet stayed")
        if wrong:
            return "FAIL", "; ".join(wrong)
        return "PASS", (f"a PIN showing, the field focused, the PIN {'shown' if reveal else 'masked'} as "
                        f"\"reveal the PIN\" says; typed: paired, and the sheet went by itself")
    finally:
        if OTHER_NAME in host_addresses(saved_hosts(ctx)):
            why = remove_second_host(ctx, cleanup=True, address=OTHER_NAME)
            if why or OTHER_NAME in host_addresses(saved_hosts(ctx)):
                print(f"note: AV2 left {OTHER_NAME} saved on the phone: {why or 'still there'}", file=sys.stderr)
        if saved_hosts(ctx) != before or phone_records(ctx) != records:
            print("note: AV2 left the phone's saved computers changed", file=sys.stderr)
        try:
            ctx.server.stop()
        except (OSError, ValueError) as error:
            print(f"note: an AV2 cleanup step failed: {error}", file=sys.stderr)
        # As AT2's: a server killed at its stop may hold a file a moment.
        missed = []
        for name, data in kept.items():
            for attempt in range(5):
                try:
                    tmp = state / f"{name}.tmp"
                    tmp.write_bytes(data)
                    os.replace(tmp, state / name)
                    break
                except OSError as error:
                    if attempt == 4:
                        missed.append(f"{name} ({error})")
                    time.sleep(1)
        if missed:
            print(f"note: AV2 could not put back {', '.join(missed)}; the server is left stopped: "
                  f"put them back in {state} by hand", file=sys.stderr)
        else:
            try:
                why = by_home(ctx)
                if why and "internet road" in why:
                    why = by_home(ctx)
                if why:
                    print(f"note: AV2 left the phone off its home link: {why}", file=sys.stderr)
            except (OSError, ValueError) as error:
                print(f"note: an AV2 cleanup step failed: {error}", file=sys.stderr)


@check("aq2-second-card", "§20")
def aq2_second_card(ctx: Ctx) -> tuple[str, str]:
    """A second computer added through Home's form, at a documentation
    address, comes first on Home (the last one asked for); removed, and
    added and removed again, the phone's saved computers and every record
    kept with them are as they were, auto-connect too (§20). Its Connect
    dials an address where nothing answers; no tap goes to a computer."""
    before = saved_hosts(ctx)
    records = phone_records(ctx)
    kept = host_addresses(before)
    if not kept:
        return "SKIP", "the phone has no saved computers"
    if SECOND_HOST in kept:
        return "SKIP", f"{SECOND_HOST} is already saved on the phone"
    # Linked to the throwaway first: its identity is what a card dialed
    # elsewhere took on (AQ3).
    if not wait_for(lambda: ctx.device in ctx.server.linked(), 90, 2):
        return "TIMEOUT", "the phone did not link to the throwaway within 90 s"
    said = []
    try:
        for step in ("added", "added again"):
            why = add_second_host(ctx)
            if why:
                return "FAIL", "; ".join(said + [why])
            now = host_addresses(saved_hosts(ctx))
            if now != [SECOND_HOST] + kept:
                where = now.index(SECOND_HOST) + 1 if SECOND_HOST in now else None
                return "FAIL", "; ".join(said + [f"{step}: {'not saved' if where is None else f'card {where} of {len(now)}'}"])
            # Nothing of another computer's taken on: no name, no WAN identity
            # (AQ3: it took the linked computer's and was dialed to it).
            taken = sorted(k.split(":")[0] for k in ctx.phone.prefs("pocketdesk-host")
                           if k.endswith(f":{SECOND_HOST}") and k.split(":")[0] in ("computer_name", "wan_identity"))
            if taken:
                return "FAIL", "; ".join(said + [f"{step}: it took another computer's {' and '.join(taken)}"])
            # Home shows it above the others, as itself.
            if not home(ctx):
                return "FAIL", "; ".join(said + [f"{step}: the app did not open on Home"])
            dump = ctx.phone.dump()
            rows = [y for text, _, y in dump_nodes(dump) if text == SECOND_HOST]
            firsts = [y for text, _, y in dump_nodes(dump) if text.startswith("127.0.0.1")]
            if not rows or (firsts and rows[0] > firsts[0]):
                return "FAIL", "; ".join(said + [f"{step}: not shown above the throwaway's card"])
            why = remove_second_host(ctx)
            if why:
                return "FAIL", "; ".join(said + [f"{step}: {why}"])
            if saved_hosts(ctx) != before:
                return "FAIL", "; ".join(said + [f"{step}, removed: the saved computers not as they were"])
            after = phone_records(ctx)
            changed = sorted({k.split(":")[0] for k in set(records) | set(after) if records.get(k) != after.get(k)})
            if changed:
                return "FAIL", "; ".join(said + [f"{step}, removed: records changed: {', '.join(changed)}"])
            said.append(f"{step}: first of {len(kept) + 1}, removed, the rest and their records as they were")
        return "PASS", "; ".join(said)
    finally:
        if SECOND_HOST in host_addresses(saved_hosts(ctx)):
            why = remove_second_host(ctx, cleanup=True)
            if why or SECOND_HOST in host_addresses(saved_hosts(ctx)):
                print(f"note: AQ2 left {SECOND_HOST} saved on the phone: {why or 'still there'}", file=sys.stderr)


# ---- someone else's phone, paired from the command line (AT1) ----

# The trust store's capabilities (core's Capability::ALL, as serialized).
CAPABILITIES = [
    "input", "clipboard", "files", "browse", "audio", "mic", "power", "exec",
    "notifications", "texts", "links", "media", "cast", "tasks", "ring", "mute",
    "away_lock", "phone_sound", "dial", "command", "phone_actions",
]
# The store keeps a capability only where it differs from its default
# (core's Capability::default_allowed): these are off unless set.
OFF_BY_DEFAULT = {"exec", "away_lock", "phone_sound"}


def link_part(value: str) -> str:
    """A pairing link's value, encoded as core's pair_link does."""
    keep = set("-._~,")
    return "".join(chr(b) if (chr(b).isascii() and chr(b).isalnum()) or chr(b) in keep else f"%{b:02X}"
                   for b in value.encode("utf-8"))


def pair_uri(name: str, device_id: str, addresses: list[str], control_port: int,
             stream_port: int, pin: str, fingerprint: Optional[str]) -> str:
    """The link the desktop's Pair page shows as a code (no WAN endpoint)."""
    parts = [
        "v=1",
        f"name={link_part(name)}",
        f"id={link_part(device_id)}",
        f"a={link_part(','.join(addresses))}",
        f"cp={control_port}",
        f"sp={stream_port}",
        f"pin={link_part(pin)}",
    ]
    if fingerprint:
        parts.append(f"fp={link_part(fingerprint)}")
    return "pocketdesk://pair?" + "&".join(parts)


def narrow_as(permissions: Optional[dict], allowed: set[str], every: list[str] = CAPABILITIES) -> Optional[str]:
    """Why `permissions` are not someone else's: only `allowed` on, every
    other capability off (one unset is its default), and sessions held to
    ask first; None when they are."""
    if permissions is None:
        return "no permissions kept for it"

    def on(c: str) -> bool:
        value = permissions.get(c)
        return value if isinstance(value, bool) else c not in OFF_BY_DEFAULT

    wrong = [c for c in every if on(c) != (c in allowed)]
    if wrong:
        return "not as someone else's: " + ", ".join(f"{c} {'on' if on(c) else 'off'}" for c in wrong)
    if permissions.get("ask_first") is not True:
        return "its sessions do not ask first"
    return None


def endpoint_id(identity: object) -> Optional[str]:
    """A WAN identity's endpoint id (the JSON the phone keeps per card)."""
    try:
        return json.loads(str(identity)).get("endpointId")
    except (ValueError, AttributeError):
        return None


def trusted_store(ctx: Ctx) -> dict:
    try:
        return json.loads((ctx.server.root / "state" / "trusted_devices.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}


def open_pair_link(ctx: Ctx, pin: str) -> Optional[str]:
    """Open a pairing link for the throwaway on the phone, as another app
    would, and confirm it there; why not, when it could not."""
    state = ctx.server.root / "state"
    phone = ctx.phone.prefs("pocketdesk-host")
    card = next((h for h in json.loads(phone.get("hosts") or "[]") if h.get("address") == "127.0.0.1"), None)
    if card is None:
        return "no 127.0.0.1 card on the phone"
    # The card's own name, so the phone's records stay as they were.
    name = str(phone.get("computer_name:127.0.0.1") or "Pocketdesk")
    try:
        device_id = (state / "device_id").read_text(encoding="utf-8").strip()
        fingerprint = hashlib.sha256((state / "certs" / "identity.der").read_bytes()).hexdigest()
    except OSError as error:
        return f"the throwaway's identity could not be read: {error}"
    uri = pair_uri(name, device_id, ["127.0.0.1"], int(card.get("control_port", 1716)),
                   int(card.get("port", 4716)), pin, fingerprint)
    ctx.phone.wake()
    am("start", "-a", "android.intent.action.VIEW", "-d", uri, APP)
    if not wait_for(lambda: shows(ctx, "Pair"), 15, 1):
        return "the link did not open the pairing screen"
    tap_text("Pair", ctx.phone.dump())
    return None


def unreachable(ctx: Ctx) -> bool:
    return node_box(ctx.phone.dump(), "Could not reach the computer") is not None


def phone_log() -> str:
    """The phone's Pocketdesk lines since the last clear: read before a
    screen dump, which clears the log (review)."""
    return adb("logcat", "-d", "-s", "PocketdeskRust:*", "Pocketdesk:*")


def keep_failure(ctx: Ctx, step: str, phone_lines: list[str], name: str = "at1_failure.txt") -> None:
    """What the phone showed and both logs said when a pairing did not
    take, kept in the throwaway's folder (never the repo) to be read."""
    phone_lines.append(phone_log())
    try:
        (ctx.server.root / name).write_text(
            f"STEP: {step}" + chr(10)
            + "SCREEN: " + " | ".join(t for t, _, _ in dump_nodes(ctx.phone.dump()) if t) + chr(10)
            + "SERVER:" + chr(10)
            + chr(10).join(l for l in ctx.server.log().splitlines() if re.search(r"(?i)pair|pin|refus|tls", l))
            + chr(10) + "PHONE:" + chr(10) + "".join(phone_lines),
            encoding="utf-8")
    except OSError:
        pass


def grant_seconds(store: dict, device: str, started: float) -> Optional[float]:
    """How long the trust store's grant for `device` runs from `started`
    (its expiry, in Unix seconds), or None when it keeps none."""
    ends = store.get("grants", {}).get(device)
    if isinstance(ends, dict):
        ends = ends.get("expires_unix")
    return ends - started if isinstance(ends, (int, float)) and not isinstance(ends, bool) else None


def leave_to_home(ctx: Ctx) -> None:
    """Back to Home from the session a pairing opens, which waits for the
    owner's Allow: with Back, as its buttons may be hidden (no tap on it)."""
    for _ in range(3):
        if shows(ctx, HOME_MARK) or (time.sleep(1) or shows(ctx, HOME_MARK)):
            return
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        time.sleep(2)


@check("at1-someone-else", "§167")
def at1_someone_else(ctx: Ctx) -> tuple[str, str]:
    """`pocketdesk pair --someone-else` and `--grant 15m`, paired from a
    link opened on the phone, give it someone else's narrow permissions:
    files and links only, its sessions asking first, a grant's trust
    ending in 15 minutes. A phone paired as mine with Links off, paired
    again as someone else's, keeps Links off (§167). The throwaway's trust
    store and device keys are put back after, as AP2 does."""
    state = ctx.server.root / "state"
    kept = {name: (state / name).read_bytes() for name in ("trusted_devices.json", "device_keys.json")
            if (state / name).exists()}
    records = phone_records(ctx)
    if ctx.device not in trusted_store(ctx).get("trusted_device_ids", []):
        return "SKIP", "the phone is not paired with the throwaway"
    # An unpaired phone is let in only while pairing is required: with it
    # off it would get the defaults, input to this computer too (review).
    config = ctx.server.root / "config" / "config.toml"
    if re.search(r"(?m)^\s*pairing_required\s*=\s*false", config.read_text(encoding="utf-8") if config.exists() else ""):
        return "SKIP", "the throwaway's config turns pairing_required off"
    said = []
    changed = False

    def pair(*args: str) -> Optional[str]:
        code, out = ctx.server.cli("pair", *args, "--json")
        # Its JSON is the last line: the log may come first.
        line = next((l for l in reversed(out.splitlines()) if l.strip().startswith("{")), "")
        try:
            pin = json.loads(line)["pin"]
        except (ValueError, KeyError, TypeError):
            return f"pocketdesk pair {' '.join(args)} said {code}, no PIN"
        lines: list[str] = []
        why = open_pair_link(ctx, str(pin))
        if why:
            return why
        narrowed = lambda: trusted_store(ctx).get("permissions", {}).get(ctx.device, {}).get("ask_first")

        def probe() -> bool:
            if narrowed():
                return True
            lines.append(phone_log())
            return unreachable(ctx)

        if not (wait_for(probe, 30, 1) and narrowed()):
            keep_failure(ctx, f"pair {' '.join(args)}", lines)
            leave_to_home(ctx)
            return f"pair {' '.join(args)}: not paired as someone else's within 30 s (at1_failure.txt)"
        leave_to_home(ctx)
        return None

    try:
        # At home first: nothing pairs over the internet road (T10a), and a
        # phone linked by it was refused before the server saw a thing (4
        # runs of 12, "Pairing needs the same network as the computer").
        for _ in range(2):
            if not iroh_loops(ctx.server.log()):
                break
            changed = True
            why = by_home(ctx)
            if why and "internet road" not in why:
                return "TIMEOUT", why
        if iroh_loops(ctx.server.log()):
            return "TIMEOUT", "the phone stayed on the internet road, where nothing pairs"
        # A phone of mine with Links off, paired again as someone else's.
        changed = True
        set_permission_live(ctx, "links", False)
        why = pair("--someone-else")
        if why:
            return "FAIL", why
        store = trusted_store(ctx)
        wrong = narrow_as(store.get("permissions", {}).get(ctx.device), {"files"})
        if wrong:
            return "FAIL", f"paired again as someone else's: {wrong}"
        said.append("paired again as someone else's: files only, Links still off, asks first")

        for args, step in ((("--someone-else",), "--someone-else"), (("--grant", "15m"), "--grant 15m")):
            code, _ = ctx.server.cli("unpair", ctx.device)
            if code != 0 or not wait_for(lambda: ctx.device not in trusted_store(ctx).get("trusted_device_ids", []), 15, 1):
                return "FAIL", "; ".join(said + [f"{step}: the phone not unpaired first"])
            started = time.time()
            why = pair(*args)
            if why:
                return "FAIL", "; ".join(said + [why])
            store = trusted_store(ctx)
            wrong = narrow_as(store.get("permissions", {}).get(ctx.device), {"files", "links"})
            if wrong:
                return "FAIL", "; ".join(said + [f"{step}: {wrong}"])
            if step == "--grant 15m":
                runs = grant_seconds(store, ctx.device, started)
                # From before the pairing: 15 minutes, and the seconds it took.
                if runs is None or not 900 <= runs <= 960:
                    return "FAIL", "; ".join(said + [f"{step}: its trust does not end in 15 minutes ({runs})"])
                said.append(f"{step}: files and links, asks first, trust for {round(runs / 60)} min")
            else:
                said.append(f"{step}: files and links, asks first")
        after = phone_records(ctx)

        def same(key: str) -> bool:
            # A session refreshes the computer's WAN identity from its own
            # link, its addresses as they are now: the same if its id is.
            if key.startswith("wan_identity:"):
                return endpoint_id(records.get(key)) == endpoint_id(after.get(key))
            return records.get(key) == after.get(key)

        moved = sorted({k.split(":")[0] for k in set(records) | set(after) if not same(k)})
        if moved:
            return "FAIL", "; ".join(said + [f"the phone's records changed: {', '.join(moved)}"])
        return "PASS", "; ".join(said)
    finally:
        if changed:
            leave_to_home(ctx)
            # As AP2's: each step whatever the one before did.
            try:
                ctx.server.stop()
            except (OSError, ValueError) as error:
                print(f"note: an AT1 cleanup step failed: {error}", file=sys.stderr)
            missed = []
            for name, data in kept.items():
                for attempt in range(5):
                    try:
                        tmp = state / f"{name}.tmp"
                        tmp.write_bytes(data)
                        os.replace(tmp, state / name)
                        break
                    except OSError as error:
                        if attempt == 4:
                            missed.append(f"{name} ({error})")
                        time.sleep(1)
            if missed:
                print(f"note: AT1 could not put back {', '.join(missed)}; the server is left stopped: "
                      f"put them back in {state} by hand", file=sys.stderr)
            else:
                try:
                    why = by_home(ctx)
                    if why and "internet road" in why:
                        why = by_home(ctx)
                    if why:
                        print(f"note: AT1 left the phone off its home link: {why}", file=sys.stderr)
                except (OSError, ValueError) as error:
                    print(f"note: an AT1 cleanup step failed: {error}", file=sys.stderr)


@check("at2-pair-from-road", "§167")
def at2_pair_from_road(ctx: Ctx) -> tuple[str, str]:
    """A phone whose link to the computer came by the internet road pairs
    from a code all the same when the computer's home address answers: the
    PIN goes by a one-off home connection, the road link left alone (AT2;
    it was refused, "Pairing needs the same network as the computer").
    Paired again as someone else's, with Links off as AT1 does; the
    throwaway's trust store and device keys are put back after."""
    config = ctx.server.root / "config" / "config.toml"
    if re.search(r"(?m)^\s*pairing_required\s*=\s*false", config.read_text(encoding="utf-8") if config.exists() else ""):
        return "SKIP", "the throwaway's config turns pairing_required off"
    state = ctx.server.root / "state"
    kept = {name: (state / name).read_bytes() for name in ("trusted_devices.json", "device_keys.json")
            if (state / name).exists()}
    if ctx.device not in trusted_store(ctx).get("trusted_device_ids", []):
        return "SKIP", "the phone is not paired with the throwaway"
    changed = True
    try:
        why = by_road(ctx)
        if why:
            return "TIMEOUT", why
        # Home in reach again, the phone's link still on the road.
        adb("reverse", f"tcp:{ctx.phone_port}", f"tcp:{ctx.port}")
        if not iroh_loops(ctx.server.log()) or ctx.device not in ctx.server.linked():
            return "TIMEOUT", "the phone's link was not on the internet road"
        set_permission_live(ctx, "links", False)
        code, out = ctx.server.cli("pair", "--someone-else", "--json")
        line = next((l for l in reversed(out.splitlines()) if l.strip().startswith("{")), "")
        try:
            pin = json.loads(line)["pin"]
        except (ValueError, KeyError, TypeError):
            return "FAIL", f"pocketdesk pair --someone-else said {code}, no PIN"
        lines: list[str] = []
        why = open_pair_link(ctx, str(pin))
        if why:
            return "FAIL", why
        narrowed = lambda: trusted_store(ctx).get("permissions", {}).get(ctx.device, {}).get("ask_first")

        def probe() -> bool:
            if narrowed():
                return True
            lines.append(phone_log())
            return unreachable(ctx)

        if not (wait_for(probe, 30, 1) and narrowed()):
            keep_failure(ctx, "pair --someone-else from the road", lines, "at2_failure.txt")
            refused = unreachable(ctx)
            leave_to_home(ctx)
            return "FAIL", ("refused: the phone could not pair while linked by the internet road"
                            if refused else "not paired as someone else's within 30 s") + " (at2_failure.txt)"
        leave_to_home(ctx)
        wrong = narrow_as(trusted_store(ctx).get("permissions", {}).get(ctx.device), {"files"})
        if wrong:
            return "FAIL", f"paired from the road: {wrong}"
        # The road link left alone (review): still the phone's link.
        if ctx.device not in ctx.server.linked():
            return "FAIL", "paired, but the phone's link by the internet road went"
        return "PASS", ("linked by the internet road, paired from a code at home: files only, Links still off, "
                        "asks first; the road link still up")
    finally:
        if changed:
            leave_to_home(ctx)
            try:
                ctx.server.stop()
            except (OSError, ValueError) as error:
                print(f"note: an AT2 cleanup step failed: {error}", file=sys.stderr)
            missed = []
            for name, data in kept.items():
                for attempt in range(5):
                    try:
                        tmp = state / f"{name}.tmp"
                        tmp.write_bytes(data)
                        os.replace(tmp, state / name)
                        break
                    except OSError as error:
                        if attempt == 4:
                            missed.append(f"{name} ({error})")
                        time.sleep(1)
            if missed:
                print(f"note: AT2 could not put back {', '.join(missed)}; the server is left stopped: "
                      f"put them back in {state} by hand", file=sys.stderr)
            else:
                try:
                    why = by_home(ctx)
                    if why and "internet road" in why:
                        why = by_home(ctx)
                    if why:
                        print(f"note: AT2 left the phone off its home link: {why}", file=sys.stderr)
                except (OSError, ValueError) as error:
                    print(f"note: an AT2 cleanup step failed: {error}", file=sys.stderr)


# ---- the HUD's delay on an idle desktop (AU2) ----


def hud_numbers(dump: str) -> Optional[tuple[int, int]]:
    """The HUD's delay and round trip, in ms, or None while either is not
    shown ("delay 23 ms", "rtt 6 ms")."""
    texts = [text for text, _, _ in dump_nodes(dump)]

    def number(name: str) -> Optional[int]:
        found = next((re.match(rf"{name} (\d+) ms$", t) for t in texts if re.match(rf"{name} \d+ ms$", t)), None)
        return int(found.group(1)) if found else None

    delay, rtt = number("delay"), number("rtt")
    return (delay, rtt) if delay is not None and rtt is not None else None


# The delay chip is an average of each frame's estimate that steps toward a
# new value by quarters, floored, so it can lag the rtt chip's median by
# a few ms (review): that much under half the round trip is not a fault.
DELAY_LAG_MS = 3


def delay_verdict(samples: list[tuple[int, int]]) -> Optional[str]:
    """Why the HUD's delay, over `samples` of (delay, rtt) once the round
    trip is measured, is not what §2b expects on an idle desktop; None when
    it is. It sits in the low tens of ms, at least about half the round
    trip (it is the frame's age plus half of it), and is not the round trip
    itself, the old bug's shape (a literal RTT proxy)."""
    if len(samples) < 4:
        return f"only {len(samples)} samples with a round trip measured"
    delays = sorted(d for d, _ in samples)
    median = delays[len(delays) // 2]
    if median >= 100:
        return f"delay's median {median} ms, not under 100 ms"
    copies = sum(1 for d, r in samples if d == r)
    if copies * 2 >= len(samples):
        return f"delay a copy of the round trip in {copies} of {len(samples)}"
    under = sum(1 for d, r in samples if (d + DELAY_LAG_MS) * 2 < r)
    if under * 5 > len(samples):
        return f"delay under half the round trip in {under} of {len(samples)}"
    return None


@check("au2-hud-delay", "§2b")
def au2_hud_delay(ctx: Ctx) -> tuple[str, str]:
    """In a session by the internet road on an idle desktop, the HUD's
    delay sits in the low tens of ms, about half the round trip or more,
    never a copy of it (§2b). On an idle desktop the frames are the
    capture's repeats, so the delay measures their encode and queue, not
    the compositor's age (review); that and heavy motion stay owed."""
    def body(ctx: Ctx) -> tuple[str, str]:
        if start_session(ctx) is None:
            return "SKIP", "no Connect on a 127.0.0.1 card on Home"
        if not wait_for(lambda: in_session(ctx), 30, 1):
            return "TIMEOUT", "no session within 30 s"
        # The round trip is measured once the caption says so (§2d): a
        # round trip under 1 ms reads "rtt 0 ms" (review).
        def measured(dump: str) -> bool:
            caption = hud_caption(dump)
            return bool(caption) and (rtt_count(caption) or 0) > 0

        if not wait_for(lambda: measured(ctx.phone.dump()), 20, 1):
            return "TIMEOUT", "no round trip measured on the HUD within 20 s"
        samples = []
        end = time.monotonic() + 20
        while time.monotonic() < end:
            dump = ctx.phone.dump()
            numbers = hud_numbers(dump)
            # A delay of 0 is before the first frame's estimate (review).
            if numbers and numbers[0] > 0 and measured(dump):
                samples.append(numbers)
            time.sleep(1)
        if len(samples) < 4:
            return "TIMEOUT", f"only {len(samples)} readings of the HUD in 20 s"
        why = delay_verdict(samples)
        delays = sorted(d for d, _ in samples)
        rtts = sorted(r for _, r in samples)
        said = (f"{len(samples)} samples: delay median {delays[len(delays) // 2]} ms "
                f"({delays[0]}-{delays[-1]}), rtt median {rtts[len(rtts) // 2]} ms") if samples else "no samples"
        return ("FAIL", f"{why}; {said}") if why else ("PASS", said)

    why = needs_details(ctx)
    if why:
        return "SKIP", why
    return session_check(ctx, body)


# ---- the phone's log, kept long enough (AC6) ----

# The phone's main log for a run. The OPPO's is 256 KiB, a couple of
# minutes of Pocketdesk's own lines: a long transfer's (AA2) were gone by
# the time a check read them.
RUN_LOG_SIZE = "16M"


def main_log_size(said: str) -> Optional[str]:
    """The main log buffer's size from `adb logcat -g`, as `-G` takes it."""
    found = re.search(r"^main: ring buffer is (\d+) ([KM])i?B", said, re.MULTILINE)
    return f"{found.group(1)}{found.group(2)}" if found else None


def kib(size: str) -> int:
    """A `-G` size ("256K", "16M") in KiB."""
    return int(size[:-1]) * (1024 if size.endswith("M") else 1)


# ---- main ----


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--server-root", type=pathlib.Path, help="a throwaway server's folder, holding state/ and config/")
    parser.add_argument("--server-exe", type=pathlib.Path, help="the pocketdesk program to run it with")
    parser.add_argument("--phone-port", type=int, default=1716, help="the control port the phone has saved for 127.0.0.1")
    parser.add_argument("--serial", help="the phone's adb serial, when more than one is attached")
    parser.add_argument("--phone-id", help="the phone's Pocketdesk id, when more than one is linked")
    parser.add_argument("--only", nargs="*", help="the checks to run")
    parser.add_argument("--list", action="store_true", help="name the checks")
    args = parser.parse_args(argv)
    # Check ids and sections print as UTF-8 on any console.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    if args.list:
        for cid, (section, _) in CHECKS.items():
            print(f"{cid:16} {section}")
        return 0
    if not args.server_root or not args.server_exe:
        parser.error("--server-root and --server-exe are needed")
    if is_owner_root(args.server_root, owner_state_dirs()):
        print("Refused: that is the owner's own state folder; give a throwaway server's.", file=sys.stderr)
        return 2
    if not (args.server_root / "state" / "trusted_devices.json").exists():
        print(f"Refused: {args.server_root} holds no paired server (state/trusted_devices.json).", file=sys.stderr)
        return 2
    if args.serial:
        os.environ["ANDROID_SERIAL"] = args.serial
    server = Server(args.server_root, args.server_exe)
    phone = Phone()
    config = args.server_root / "config" / "config.toml"
    port = control_port(config.read_text(encoding="utf-8") if config.exists() else "")
    # Never at the owner's own server (N1 review: a copied config pointed the
    # phone there).
    if port == owner_control_port():
        print(f"Refused: the throwaway's control port {port} is the owner's; give it another in its config.toml.", file=sys.stderr)
        return 2
    had = adb("reverse", "--list")
    if f"tcp:{args.phone_port} " in had:
        print(f"Refused: adb already reverses tcp:{args.phone_port}; remove it first.", file=sys.stderr)
        return 2
    failed = False
    # A bigger main log for the run, put back after (AC6).
    log_size = main_log_size(adb("logcat", "-b", "main", "-g"))
    # Never smaller than it was (review): one already this big is left.
    if log_size and kib(log_size) < kib(RUN_LOG_SIZE):
        adb("logcat", "-b", "main", "-G", RUN_LOG_SIZE)
    else:
        log_size = None
    try:
        why = server.start()
        if why:
            print(f"TIMEOUT  the throwaway server: {why}")
            return 1
        adb("reverse", f"tcp:{args.phone_port}", f"tcp:{port}")
        phone.wake()
        linked = wait_for(server.linked, 90, 2)
        if not linked:
            print("TIMEOUT  the phone never linked to the throwaway server")
            return 1
        device = args.phone_id or linked[0]
        if device not in linked:
            print(f"TIMEOUT  {device} is not linked")
            return 1
        ctx = Ctx(server, phone, device, args.phone_port, port)
        for cid, (section, fn) in CHECKS.items():
            if args.only and cid not in args.only:
                continue
            if cid in LONG and not (args.only and cid in args.only):
                continue
            try:
                status, detail = fn(ctx)
            except Exception as error:  # a check's own failure is a FAIL, not the run's end
                status, detail = "FAIL", f"{type(error).__name__}: {error}"
            failed |= status in ("FAIL", "TIMEOUT")
            print(f"{status:8} {cid:16} {section:6} {detail}", flush=True)
    finally:
        server.stop()
        adb("reverse", "--remove", f"tcp:{args.phone_port}")
        if log_size:
            # A size set again empties the log: what the run saw is kept
            # in the throwaway's folder first, for a failure to be read.
            # Kept in the throwaway's folder only, never in the repo: it is
            # the phone's whole log.
            try:
                with open(args.server_root / "phone_log.txt", "w", encoding="utf-8") as kept:
                    kept.write(adb("logcat", "-b", "main", "-d", "-v", "time", timeout=120))
            except OSError as error:
                print(f"note: the phone's log was not kept: {error}", file=sys.stderr)
            adb("logcat", "-b", "main", "-G", log_size)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
