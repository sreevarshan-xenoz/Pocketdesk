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
import html
import http.client
import json
import os
import pathlib
import re
import secrets
import shlex
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass
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

    def env(self) -> dict:
        return {
            "COMPUTERNAME": "Test Desk",
            "POCKETDESK_STATE_DIR": str(self.root / "state"),
            "POCKETDESK_CONFIG_DIR": str(self.root / "config"),
            "POCKETDESK_CLIPBOARD_FILE": str(self.root / "clip.txt"),
            # N5: what plays here, read from a file, so this box's players are untouched.
            "POCKETDESK_MEDIA_FILE": str(self.root / "computer_media.json"),
            # P3: presenting while this file is there.
            "POCKETDESK_PRESENTING_FILE": str(self.root / "presenting"),
            # P7: the files copied here, one path a line.
            "POCKETDESK_CLIPBOARD_FILES_FILE": str(self.root / "clip_files.txt"),
            # X1 review: what phones send lands in the throwaway's own folder,
            # whatever its config says, never this computer's Downloads.
            "POCKETDESK_DOWNLOAD_DIR": str(self.root / "downloads"),
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
            store.setdefault("permissions", {}).setdefault(device, {})["input"] = False
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
        if not path.exists():
            return []
        return fresh_linked(path.read_text(encoding="utf-8"), time.time())

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
    spot = wait_for(lambda: connect_button(ctx.phone.dump(), "127.0.0.1"), 20, 2)
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
        share_to_throwaway(name)
        landed = ctx.server.root / "downloads" / name
        size = local.stat().st_size
        if not wait_for(lambda: landed.exists() and landed.stat().st_size == size, deadline, 2):
            return None, f"not in the throwaway's downloads at full size within {deadline:.0f} s"
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
            drop_from_phone(path.name)
            path.unlink(missing_ok=True)
    return session_check(ctx, body)


LONG.add("x2-resume")


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
        back = sum(f"({len(token)} chars)" in line and "Clipboard sent to" in line
                   for line in adb("logcat", "-d", "-s", "PocketdeskRust:*").splitlines())
        kept = ctx.server.clipboard() == token
        said = f"sent to the phone {sent} time(s); the phone sent it back {back} time(s); the computer's copy {'kept' if kept else 'changed'}"
        return ("PASS" if sent == 1 and back == 0 and kept else "FAIL"), said
    return session_check(ctx, body)


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
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
