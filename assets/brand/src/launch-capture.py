"""Capture launch-video.html frame by frame in headless Chrome, then put the
frames and soundtrack.wav together as the launch video, its poster and the
README's animated preview.

Every frame is a function of time (the page's `render(t)`), so the capture
is exact at any frame rate. Usage, from this folder:

    python launch-music.py          # writes soundtrack.wav
    python launch-capture.py <chrome.exe> <out folder>

Needs websocket-client and ffmpeg on PATH. Writes pocketdesk-launch.mp4,
pocketdesk-launch.jpg and pocketdesk-launch.gif in <out folder>.
"""
import base64
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

import websocket

FPS = 30
PORT = 9333
HERE = pathlib.Path(__file__).resolve().parent


def main():
    chrome, out = sys.argv[1], pathlib.Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    frames = pathlib.Path(tempfile.mkdtemp(prefix="pd-frames-"))
    profile = tempfile.mkdtemp(prefix="pd-chrome-")
    proc = subprocess.Popen([
        chrome, "--headless=new", f"--remote-debugging-port={PORT}", f"--user-data-dir={profile}",
        "--window-size=1920,1080", "--hide-scrollbars", "--force-device-scale-factor=1",
        "--no-first-run", "--no-default-browser-check", "--remote-allow-origins=*", "about:blank",
    ])
    try:
        for _ in range(100):
            try:
                tabs = json.load(urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json"))
                page = next(t for t in tabs if t.get("type") == "page")
                break
            except Exception:
                time.sleep(0.2)
        else:
            raise SystemExit("Chrome did not open its debugging port")
        ws = websocket.create_connection(page["webSocketDebuggerUrl"], max_size=None)
        ids = iter(range(1, 10**9))

        def call(method, **params):
            i = next(ids)
            ws.send(json.dumps({"id": i, "method": method, "params": params}))
            while True:
                msg = json.loads(ws.recv())
                if msg.get("id") == i:
                    if "error" in msg:
                        raise RuntimeError(f"{method}: {msg['error']}")
                    return msg.get("result", {})

        call("Emulation.setDeviceMetricsOverride", width=1920, height=1080, deviceScaleFactor=1, mobile=False)
        call("Page.enable")
        call("Page.navigate", url=(HERE / "launch-video.html").as_uri())
        time.sleep(2)
        call("Runtime.evaluate", expression="document.fonts.ready.then(() => true)", awaitPromise=True)
        dur = call("Runtime.evaluate", expression="window.DUR", returnByValue=True)["result"]["value"]
        count = int(round(dur * FPS))
        for i in range(count):
            call("Runtime.evaluate", expression=f"render({i / FPS})")
            shot = call("Page.captureScreenshot", format="png")
            (frames / f"f{i:05d}.png").write_bytes(base64.b64decode(shot["data"]))
            if i % 100 == 0:
                print(f"frame {i}/{count}", flush=True)
        ws.close()
    finally:
        proc.terminate()

    wav = HERE / "soundtrack.wav"
    mp4 = out / "pocketdesk-launch.mp4"
    subprocess.run([
        "ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", str(frames / "f%05d.png"),
        "-i", str(wav), "-c:v", "libx264", "-preset", "slow", "-crf", "20", "-pix_fmt", "yuv420p",
        "-movflags", "+faststart", "-c:a", "aac", "-b:a", "160k", "-shortest", str(mp4),
    ], check=True)
    # The poster: the wordmark scene, settled.
    shutil.copy(frames / f"f{int(5.4 * FPS):05d}.png", frames / "poster.png")
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(frames / "poster.png"),
                    "-q:v", "3", str(out / "pocketdesk-launch.jpg")], check=True)
    # The README's preview: it plays on the page (GitHub plays no video
    # file from a repository), 800 px wide at 12 fps, without sound.
    palette = frames / "palette.png"
    scale = "fps=12,scale=800:-1:flags=lanczos"
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(mp4), "-vf",
                    f"{scale},palettegen=max_colors=160:stats_mode=diff", str(palette)], check=True)
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(mp4), "-i", str(palette),
                    "-lavfi", f"{scale}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle",
                    "-loop", "0", str(out / "pocketdesk-launch.gif")], check=True)
    shutil.rmtree(frames, ignore_errors=True)
    shutil.rmtree(profile, ignore_errors=True)
    for f in ("pocketdesk-launch.mp4", "pocketdesk-launch.jpg", "pocketdesk-launch.gif"):
        print(f, os.path.getsize(out / f))


if __name__ == "__main__":
    main()
