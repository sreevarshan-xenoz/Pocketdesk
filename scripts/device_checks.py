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
from dataclasses import dataclass
from typing import Callable, Optional

APP = "dev.pocketdesk.android"
FIXTURE = "dev.pocketdesk.fixture"
DUMP = "/data/local/tmp/device_checks.xml"
# How long Pocketdesk's control service takes to come back after a screen read.
SETTLE = 4.0


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
    """Where the owner's own server keeps its state, never to be used here."""
    dirs = []
    if os.environ.get("LOCALAPPDATA"):
        dirs.append(pathlib.Path(os.environ["LOCALAPPDATA"]) / "pocketdesk")
    if os.environ.get("XDG_STATE_HOME"):
        dirs.append(pathlib.Path(os.environ["XDG_STATE_HOME"]) / "pocketdesk")
    dirs.append(pathlib.Path.home() / ".local" / "state" / "pocketdesk")
    return dirs


def is_owner_root(root: pathlib.Path, owners: list[pathlib.Path]) -> bool:
    """Whether `root`'s state folder is the owner's own."""
    state = (root / "state").resolve()
    return any(state == o.resolve() or root.resolve() == o.resolve() for o in owners)


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
        adb("shell", "wm", "dismiss-keyguard")
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
        return prefs(adb("shell", f"run-as {APP} cat shared_prefs/{name}.xml"))

    def accessibility_on(self) -> bool:
        return f"{APP}/{APP}.control.ControlService" in adb("shell", "settings", "get", "secure", "enabled_accessibility_services")

    def dump(self) -> str:
        """The screen's views. `uiautomator` suppresses other Accessibility
        services while it reads, so Pocketdesk's control service is away for
        a moment after: see [service_back]."""
        adb("shell", "uiautomator", "dump", DUMP)
        return adb("shell", "cat", DUMP)

    def service_back(self, timeout: float = 15) -> bool:
        """Pocketdesk's control service bound again after a dump."""
        back = wait_for(lambda: "label=Pocketdesk control" in adb("shell", "dumpsys", "accessibility"), timeout)
        # Bound is not yet connected: on the OPPO the service came back about
        # 3 s after a read (its "control service connected" in logcat).
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

    def notify(self, nid: int, title: str, text: str, style: str = "plain") -> None:
        am("broadcast", "-n", f"{FIXTURE}/.NotifyReceiver", "-a", "dev.pocketdesk.fixture.NOTIFY",
           "--ei", "id", str(nid), "--es", "title", title, "--es", "text", text, "--es", "style", style)

    def logcat(self, tag: str) -> str:
        return adb("logcat", "-d", "-s", f"{tag}:*")


@dataclass
class Server:
    root: pathlib.Path
    exe: pathlib.Path
    started: bool = False

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
        """No phone gives this computer input during the checks."""
        path = self.root / "state" / "trusted_devices.json"
        store = json.loads(path.read_text(encoding="utf-8"))
        for device in store.get("trusted_device_ids", []):
            store.setdefault("permissions", {}).setdefault(device, {})["input"] = False
        path.write_text(json.dumps(store, indent=2), encoding="utf-8")

    def start(self) -> None:
        if self.running():
            return
        self.input_off()
        (self.root / "state" / "live_sessions.json").unlink(missing_ok=True)
        log = open(self.root / "device_checks.log", "w", encoding="utf-8")
        full = dict(os.environ)
        full.update(self.env())
        subprocess.Popen([str(self.exe), "start"], stdout=log, stderr=subprocess.STDOUT, env=full)
        self.started = bool(wait_for(self.running, 30, 1))

    def stop(self) -> None:
        if self.started:
            run([str(self.exe), "stop"], env=self.env())

    def linked(self) -> list[str]:
        path = self.root / "state" / "live_sessions.json"
        return linked_ids(path.read_text(encoding="utf-8")) if path.exists() else []

    def events(self, after: int, kind: str) -> list[dict]:
        path = self.root / "state" / "events.jsonl"
        if not path.exists():
            return []
        return events_after(path.read_text(encoding="utf-8").splitlines(), after, kind)

    def clipboard(self) -> str:
        path = self.root / "clip.txt"
        return path.read_text(encoding="utf-8") if path.exists() else ""

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
    if not ctx.phone.service_back():
        return "TIMEOUT", "Pocketdesk control did not come back after the screen was read"
    ctx.server.type_batch(ctx.device, 1, [{"text": "Héllo தமிழ்"}, {"key": "backspace"}, {"text": "!"}])
    want = "Héllo தமிழ!"
    # Read once the batch has had time: each read takes the service away.
    time.sleep(4)
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
    return ("PASS", "nothing typed while locked") if got == "keep" else ("FAIL", f"field holds {got!r}")


@check("m10-copy", "§175")
def m10_copy(ctx: Ctx) -> tuple[str, str]:
    if ctx.phone.sdk() < 33:
        return "SKIP", "Android 13 or later"
    if (why := needs(ctx, "copies_by_themselves")):
        return "SKIP", why
    ctx.phone.wake()
    token = "copy-" + secrets.token_hex(4)
    ctx.phone.copy(token)
    got = wait_for(lambda: ctx.server.clipboard() == token, 20)
    return ("PASS", f"{token} reached the computer") if got else ("TIMEOUT", f"clipboard holds {ctx.server.clipboard()!r}")


@check("m10-sensitive", "§175")
def m10_sensitive(ctx: Ctx) -> tuple[str, str]:
    if ctx.phone.sdk() < 33:
        return "SKIP", "Android 13 or later"
    if (why := needs(ctx, "copies_by_themselves")):
        return "SKIP", why
    ctx.phone.wake()
    token = "secret-" + secrets.token_hex(4)
    ctx.phone.copy(token, sensitive=True)
    time.sleep(8)
    return ("FAIL", "a sensitive copy reached the computer") if ctx.server.clipboard() == token else ("PASS", "not sent")


def forwarding(ctx: Ctx) -> Optional[str]:
    notes = ctx.phone.prefs("pocketdesk-phone-notifications")
    if not notes.get("enabled") or FIXTURE not in (notes.get("shared") or []):
        return f"turn on This phone's notifications for {FIXTURE}"
    if not ctx.phone.prefs("pocketdesk-ui").get("quiet_in_hand"):
        return "turn on Quiet on the computer while I use the phone"
    return None


@check("m6-in-hand", "§170")
def m6_in_hand(ctx: Ctx) -> tuple[str, str]:
    if (why := forwarding(ctx)):
        return "SKIP", why
    ctx.phone.wake()
    since = int(time.time())
    ctx.phone.notify(601, "Ann", "In hand " + secrets.token_hex(2))
    got = wait_for(lambda: ctx.server.events(since, "notification"), 25)
    if not got:
        return "TIMEOUT", "no notification reached the computer"
    return ("PASS", "held: phone_in_use") if got[-1].get("phone_in_use") else ("FAIL", "shown while in hand")


@check("m6-locked", "§170")
def m6_locked(ctx: Ctx) -> tuple[str, str]:
    if (why := forwarding(ctx)):
        return "SKIP", why
    ctx.phone.sleep()
    since = int(time.time())
    ctx.phone.notify(602, "Ann", "Locked " + secrets.token_hex(2))
    got = wait_for(lambda: ctx.server.events(since, "notification"), 30)
    ctx.phone.wake()
    if not got:
        return "TIMEOUT", "no notification reached the computer"
    return ("FAIL", "held while locked") if got[-1].get("phone_in_use") else ("PASS", "shown")


@check("m8-tel", "§173")
def m8_tel(ctx: Ctx) -> tuple[str, str]:
    adb("logcat", "-c")
    code, out = run([str(ctx.server.exe), "tel", "tel:+1-555-0100"], env=ctx.server.env())
    if code != 0 or "Calling +15550100" not in out:
        return "FAIL", out.strip().splitlines()[-1] if out.strip() else f"exit {code}"
    got = wait_for(lambda: "number from the computer" in ctx.phone.logcat("Dials") or "Dial" in ctx.phone.logcat("Dials"), 30)
    return ("PASS", "chose the linked phone; the dial reached it") if got else ("TIMEOUT", "the dial did not reach the phone")


# ---- main ----


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--server-root", type=pathlib.Path, help="a throwaway server's folder, holding state/ and config/")
    parser.add_argument("--server-exe", type=pathlib.Path, help="the pocketdesk program to run it with")
    parser.add_argument("--phone-port", type=int, default=1716, help="the control port the phone has saved for 127.0.0.1")
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
    server = Server(args.server_root, args.server_exe)
    phone = Phone()
    port = control_port((args.server_root / "config" / "config.toml").read_text(encoding="utf-8")
                        if (args.server_root / "config" / "config.toml").exists() else "")
    adb("reverse", f"tcp:{args.phone_port}", f"tcp:{port}")
    server.start()
    failed = False
    try:
        if not server.running():
            print("TIMEOUT  the throwaway server did not start")
            return 1
        phone.wake()
        linked = wait_for(server.linked, 90, 2)
        if not linked:
            print("TIMEOUT  the phone never linked to the throwaway server")
            return 1
        ctx = Ctx(server, phone, linked[0])
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
