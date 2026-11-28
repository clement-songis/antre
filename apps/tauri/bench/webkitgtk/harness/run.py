"""Run the bench pages in WebKitGTK (MiniBrowser) and Chromium under Xvfb.

Inside the container built from ../Dockerfile, with /bench/dist (the bundled
bench pages) and /bench/out mounted:

    run.py [--engines webkitgtk,chromium] [--pages stream,transcript,canvas]
           [--runs 3] [--click-on STATUS] [--tag NAME] [--use-display]

Outside the container, set BENCH_DIST and BENCH_OUT, and pass --use-display
to run on the session's X11 display instead of a private Xvfb.

The pages keep their result in `window.__benchResult`; the HTTP server
injects a small script that POSTs it back, so no WebDriver is needed and both
engines are driven the same way. `--click-on STATUS` sends a real X11 click
to the middle of the window when the page's status line first contains
STATUS (used to check whether a user gesture lifts iframe throttling).
"""

import argparse
import glob
import json
import os
import subprocess
import threading
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

DIST = os.environ.get("BENCH_DIST", "/bench/dist")
OUT = os.environ.get("BENCH_OUT", "/bench/out")
PORT = 8765
DISPLAY = ":99"
WIDTH, HEIGHT = 1280, 860
RESULT_TIMEOUT_S = 300

INJECT = b"""<script>(function(){let sent=false,last="";setInterval(function(){
var s=document.getElementById("bench-status"),t=s?s.textContent:"";
if(t!==last){last=t;fetch("/log",{method:"POST",body:t}).catch(function(){});}
if(!sent&&window.__benchResult){sent=true;fetch("/result",{method:"POST",
headers:{"content-type":"application/json"},body:JSON.stringify(window.__benchResult)});}
},250);})();</script></body>"""


class State:
    def __init__(self):
        self.result = None
        self.done = threading.Event()
        self.on_status = None


STATE = State()


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=DIST, **kwargs)

    def log_message(self, *_):
        pass

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if not path.endswith(".html"):
            return super().do_GET()
        try:
            with open(os.path.join(DIST, path.lstrip("/")), "rb") as page:
                body = page.read().replace(b"</body>", INJECT, 1)
        except OSError:
            return self.send_error(404)
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.send_response(204)
        self.end_headers()
        if self.path == "/result":
            STATE.result = json.loads(body)
            STATE.done.set()
        elif self.path == "/log":
            text = body.decode("utf-8", "replace")
            print(f"    status: {text}", flush=True)
            if STATE.on_status:
                STATE.on_status(text)


def processes():
    text = subprocess.run(
        ["ps", "-eo", "pid=,ppid=,rss=,args="], capture_output=True, text=True
    ).stdout
    rows = []
    for line in text.splitlines():
        parts = line.split(None, 3)
        if len(parts) == 4:
            rows.append((int(parts[0]), int(parts[1]), int(parts[2]) / 1024, parts[3]))
    return rows


def descendants(root, rows):
    children = {}
    for pid, ppid, _, _ in rows:
        children.setdefault(ppid, []).append(pid)
    seen, stack = set(), [root]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        stack.extend(children.get(pid, []))
    return seen


def role(engine, args):
    if engine == "webkitgtk":
        if "WebKitWebProcess" in args:
            return "webContent"
        if "WebKitNetworkProcess" in args:
            return "networking"
        if "WebKitGPUProcess" in args:
            return "gpu"
        return "ui"
    if "--type=renderer" in args:
        return "renderer"
    if "--type=gpu-process" in args:
        return "gpu"
    return "browser"


def minibrowser():
    found = glob.glob("/usr/lib/*/webkit2gtk-4.1/MiniBrowser")
    if not found:
        raise SystemExit("MiniBrowser not found")
    return found[0]


def command(engine, url, run_id):
    if engine == "webkitgtk":
        return [minibrowser(), url]
    return [
        "chromium",
        "--no-sandbox",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-features=Translate,MediaRouter",
        f"--user-data-dir=/tmp/chromium-{run_id}",
        "--window-position=0,0",
        f"--window-size={WIDTH},{HEIGHT}",
        f"--app={url}",
    ]


def click_center():
    subprocess.run(
        ["xdotool", "mousemove", str(WIDTH // 2), str(HEIGHT // 2 + 40), "click", "1"],
        env={**os.environ, "DISPLAY": DISPLAY},
        check=False,
    )


def run_once(engine, page, run, click_on, tag):
    STATE.result = None
    STATE.done.clear()
    clicked = threading.Event()

    def on_status(text):
        if click_on and not clicked.is_set() and click_on in text:
            clicked.set()
            threading.Timer(0.3, click_center).start()

    STATE.on_status = on_status
    url = f"http://127.0.0.1:{PORT}/{page}.html?engine={engine}"
    run_id = f"{engine}-{page}-{run}"
    started = time.time()
    child = subprocess.Popen(
        command(engine, url, run_id),
        env={**os.environ, "DISPLAY": DISPLAY},
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if engine == "webkitgtk":
        # Xvfb has no window manager, so MiniBrowser keeps its 1024x768
        # default; size it like the Chromium window before the page starts.
        subprocess.Popen(
            ["xdotool", "search", "--sync", "--onlyvisible", "--pid", str(child.pid), "--class", "MiniBrowser",
             "windowmove", "0", "0", "windowsize", str(WIDTH), str(HEIGHT)],
            env={**os.environ, "DISPLAY": DISPLAY},
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    peaks = {}
    tree = set()
    while not STATE.done.wait(0.2):
        if time.time() - started > RESULT_TIMEOUT_S or child.poll() is not None:
            break
        rows = processes()
        tree = descendants(child.pid, rows)
        totals = {}
        for pid, _, rss, args in rows:
            if pid in tree:
                key = role(engine, args)
                totals[key] = totals.get(key, 0) + rss
        for key, value in totals.items():
            peaks[key] = max(peaks.get(key, 0), round(value))
    wall_ms = round((time.time() - started) * 1000)
    child.terminate()
    try:
        child.wait(10)
    except subprocess.TimeoutExpired:
        child.kill()
    # Helper processes can outlive the UI process for a moment; only the
    # ones this run started are killed (never the session's other WebViews).
    for pid in tree - {child.pid}:
        try:
            os.kill(pid, 15)
        except OSError:
            pass
    if STATE.result is None:
        print(f"  {run_id}: no result after {wall_ms} ms", flush=True)
        return
    record = {
        "engine": engine,
        "page": page,
        "run": run,
        "clickOn": click_on,
        "tag": tag,
        "env": {key: value for key, value in os.environ.items() if key.startswith("WEBKIT_")},
        "wallMs": wall_ms,
        "peakRssMb": peaks,
        "result": STATE.result,
    }
    suffix = f"-{tag}" if tag else ""
    with open(os.path.join(OUT, f"{run_id}{suffix}.json"), "w") as out:
        json.dump(record, out, indent=1)
    print(f"  {run_id}{suffix}: ok in {wall_ms} ms, rss {peaks}", flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--engines", default="webkitgtk,chromium")
    parser.add_argument("--pages", default="stream,transcript,canvas")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--click-on", default=None)
    parser.add_argument("--tag", default=None, help="file suffix; defaults to 'click' with --click-on")
    parser.add_argument("--use-display", action="store_true")
    options = parser.parse_args()
    tag = options.tag or ("click" if options.click_on else None)
    os.makedirs(OUT, exist_ok=True)

    global DISPLAY
    xvfb = None
    if options.use_display:
        DISPLAY = os.environ.get("DISPLAY") or ":0"
    else:
        xvfb = subprocess.Popen(
            ["Xvfb", DISPLAY, "-screen", "0", f"{WIDTH}x{HEIGHT}x24", "-nolisten", "tcp"],
            stderr=subprocess.DEVNULL,
        )
        for _ in range(50):
            if os.path.exists(f"/tmp/.X11-unix/X{DISPLAY[1:]}"):
                break
            time.sleep(0.1)
        bus = subprocess.run(
            ["dbus-daemon", "--session", "--fork", "--print-address"],
            capture_output=True,
            text=True,
        ).stdout.strip()
        if bus:
            os.environ["DBUS_SESSION_BUS_ADDRESS"] = bus

    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        for engine in options.engines.split(","):
            for page in options.pages.split(","):
                for run in range(1, options.runs + 1):
                    print(f"{engine} {page} run {run}", flush=True)
                    run_once(engine, page, run, options.click_on, tag)
    finally:
        server.shutdown()
        if xvfb:
            xvfb.terminate()


if __name__ == "__main__":
    main()
