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


def field_text(dump: str, description: str = "fixture-field") -> Optional[str]:
    """The text of the view `uiautomator dump` names `description`."""
    for node in re.findall(r"<node [^>]*>", dump):
        if f'content-desc="{description}"' in node:
            found = re.search(r' text="([^"]*)"', node)
            return html.unescape(found.group(1)) if found else ""
    return None


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

    def notify(self, nid: int, title: str, text: str, style: str = "plain") -> None:
        am("broadcast", "-n", f"{FIXTURE}/.NotifyReceiver", "-a", "dev.pocketdesk.fixture.NOTIFY",
           "--ei", "id", str(nid), "--es", "title", title, "--es", "text", text, "--es", "style", style)

    def logcat(self, tag: str) -> str:
        return adb("logcat", "-d", "-s", f"{tag}:*")


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
        }

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

    def clipboard(self) -> str:
        path = self.root / "clip.txt"
        return path.read_text(encoding="utf-8") if path.exists() else ""

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
        ctx = Ctx(server, phone, device)
        for cid, (section, fn) in CHECKS.items():
            if args.only and cid not in args.only:
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
